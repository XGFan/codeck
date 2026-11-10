//! The Host's HTTP + WebSocket face on the LAN. The daemon always serves it; the desktop app when 「局域网访问」 is on
//! (ADR-0012), which is what [`LanService`] switches.

pub mod net;
pub mod server;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::Utf8Bytes;
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use self::net::Subnet;
use self::server::{Ctx, Gate, Serving};
use crate::Host;
use crate::ccrpc::relay;
use crate::commands::{CmdResult, internal};

/// The daemon's and the desktop app's port.
pub const DEFAULT_PORT: u16 = 38471;
/// How often the listeners are checked against the machine's addresses.
const NETWORK_POLL: Duration = Duration::from_secs(5);

/// The addresses to listen on now, or why they cannot be read.
pub type Subnets = Arc<dyn Fn() -> Result<Vec<Subnet>, String> + Send + Sync>;

/// What a Host needs to serve the LAN when asked to.
pub struct LanConfig {
    /// What the WebSocket clients read ([`server::event_channel`]).
    pub events: broadcast::Sender<Utf8Bytes>,
    /// The static files under `/web/`.
    pub web_dir: PathBuf,
    pub port: u16,
    pub subnets: Subnets,
}

impl LanConfig {
    /// Listening on every LAN interface's private address.
    pub fn new(events: broadcast::Sender<Utf8Bytes>, web_dir: PathBuf, port: u16) -> Self {
        LanConfig {
            events,
            web_dir,
            port,
            subnets: Arc::new(|| {
                let ifaces = net::interfaces().map_err(|e| format!("读不了网络接口：{e}"))?;
                // None yet is not an error: the network may come up later.
                Ok(net::listeners(&[], &ifaces).unwrap_or_default())
            }),
        }
    }
}

/// 「局域网访问」 as the settings page shows it.
#[derive(Serialize, specta::Type, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LanStatus {
    pub enabled: bool,
    /// `address:port`, each being listened on.
    pub addresses: Vec<String>,
    /// Why an address that should be listened on is not.
    pub error: Option<String>,
}

/// The on/off state of a Host's LAN access.
#[derive(Default)]
pub(crate) struct LanService {
    /// Read when the window closes, which cannot wait for `running`.
    enabled: AtomicBool,
    /// Serializes turning it on and off.
    running: tokio::sync::Mutex<Option<Running>>,
}

struct Running {
    gate: Gate,
    status: Arc<Mutex<LanStatus>>,
    task: JoinHandle<()>,
}

/// Serves what `subnets` says now and records the outcome.
async fn follow(serving: &mut Serving, subnets: &Subnets, status: &Mutex<LanStatus>) {
    let read = subnets();
    if let Ok(wanted) = &read {
        serving.follow(wanted).await;
    }
    let mut s = status.lock();
    s.addresses = serving.listening();
    let mut why = serving.errors();
    why.extend(read.err());
    s.error = (!why.is_empty()).then(|| why.join("；"));
}

impl Host {
    /// Whether LAN access is on (the window then only hides when closed).
    pub fn lan_enabled(&self) -> bool {
        self.lan.enabled.load(Ordering::SeqCst)
    }

    pub async fn lan_status(&self) -> CmdResult<LanStatus> {
        let running = self.lan.running.lock().await;
        Ok(running
            .as_ref()
            .map_or_else(LanStatus::default, |r| r.status.lock().clone()))
    }

    pub async fn lan_set_enabled(&self, on: bool) -> CmdResult<LanStatus> {
        let Some(cfg) = &self.config.lan else {
            return Err(relay::invalid("这个 Host 没有局域网访问的开关"));
        };
        tokio::task::spawn_blocking(move || crate::settings::update(|s| s.lan_access = on))
            .await
            .map_err(internal)?
            .map_err(|e| relay::io(format!("保存设置失败：{e}")))?;
        self.lan_switch(cfg, on).await;
        self.lan_status().await
    }

    /// Turns LAN access on if the settings say so (at startup).
    pub async fn lan_restore(&self) {
        let saved = tokio::task::spawn_blocking(|| crate::settings::load().lan_access).await;
        if let (Ok(true), Some(cfg)) = (saved, &self.config.lan) {
            self.lan_switch(cfg, true).await;
        }
    }

    async fn lan_switch(&self, cfg: &LanConfig, on: bool) {
        let mut running = self.lan.running.lock().await;
        // The list of Sessions must stay current for the LAN's clients while the window is hidden. A Host that keeps
        // its watch from the start has nothing to switch.
        let hub = self.connect.clone().filter(|_| !self.config.keep_ls_watch);
        if on && running.is_none() {
            *running = Some(self.lan_start(cfg).await);
            if let Some(hub) = hub {
                self.ccrpc.keep_watching(hub, self.events.clone());
            }
        } else if !on && let Some(r) = running.take() {
            r.gate.close();
            // Over when the port is free again.
            let _ = r.task.await;
            if let Some(hub) = hub {
                self.ccrpc.stop_keeping(hub, self.events.clone()).await;
            }
        }
        self.lan.enabled.store(on, Ordering::SeqCst);
    }

    /// Listens now, so the status it leaves is accurate, and goes on following the network until the gate closes.
    async fn lan_start(&self, cfg: &LanConfig) -> Running {
        let host = self.me.upgrade().expect("a Host is made by Host::start");
        let gate = Gate::default();
        let ctx = Ctx {
            host,
            events: cfg.events.clone(),
            allowed_origins: Arc::default(),
            web_dir: cfg.web_dir.clone(),
            gate: gate.clone(),
        };
        let mut serving = Serving::new(cfg.port, Box::new(move |s| server::router(ctx.clone(), s)));
        let status = Arc::new(Mutex::new(LanStatus {
            enabled: true,
            ..LanStatus::default()
        }));
        let subnets = cfg.subnets.clone();
        follow(&mut serving, &subnets, &status).await;
        let task = {
            let (gate, status) = (gate.clone(), status.clone());
            tokio::spawn(async move {
                let mut poll = tokio::time::interval(NETWORK_POLL);
                // The first tick is immediate, and just done.
                poll.tick().await;
                loop {
                    tokio::select! {
                        _ = gate.closed() => break,
                        _ = poll.tick() => follow(&mut serving, &subnets, &status).await,
                    }
                }
                serving.stop().await;
            })
        };
        Running { gate, status, task }
    }
}
