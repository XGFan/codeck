//! CoDeck's Host (ADR-0011): the runtime around `codeck_core::Engine` (background worker, watcher, Live
//! polling, settings, prices, ccrpc) and the one method table both the desktop app and the daemon serve.
//! No tauri here: events go to an `EventSink`, paths come from `HostConfig`.

pub mod ccrpc;
pub mod commands;
pub mod connect;
pub mod events;
pub mod lan;
mod methods;
pub mod prices;
pub mod settings;
pub mod state;
pub mod watcher;
pub mod worker;

#[cfg(test)]
#[path = "../../codeck-core/tests/common/fixtures.rs"]
mod fixtures;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use codeck_core::{CoreResult, paths};

pub use crate::events::{EventSink, HostEvent};
pub use crate::methods::dispatch;
use crate::state::{AppState, open_engine};

/// The types the method table names (`for_each_method!` expands to `$crate::types::…`).
pub mod types {
    pub use crate::ccrpc::relay::{AnalyzerRecords, AnalyzerTest};
    pub use crate::ccrpc::{AnalyzerInfo, CcrpcInfo};
    pub use crate::lan::LanStatus;
    pub use crate::settings::{LaunchRule, SidebarProject};
    pub use codeck_core::model::*;
}

/// Shows a path in the file manager, or opens it; `Err` is why not.
pub type PathAction = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// Where a Host keeps its state, and what it can do on the machine it runs on.
pub struct HostConfig {
    /// The index and the cached price list.
    pub cache_dir: PathBuf,
    /// settings.json.
    pub settings_path: PathBuf,
    /// ccrpc's logs; `None` = `~/Library/Logs/dev.joy.codeck/ccrpc` under the login shell's `HOME`.
    pub ccrpc_logs: Option<PathBuf>,
    /// Pasted images; `None` = `~/Library/Caches/dev.joy.codeck/images` under the login shell's `HOME`.
    pub images: Option<PathBuf>,
    /// Data root over the settings' (`--data-root`); `None` = settings > `$CLAUDE_CONFIG_DIR` > `~/.claude`.
    pub data_root: Option<PathBuf>,
    /// `reveal_session_file`; `None` = not available on this Host.
    pub reveal_path: Option<PathAction>,
    /// `ccrpc_open_analyzer_records`; `None` = not available on this Host.
    pub open_path: Option<PathAction>,
    /// Reduce ccrpc's lines to Connect state in the Host (ADR-0011): the `connect_*` / `ccrpc_ls` methods and
    /// `ConnectState` events. The desktop app's front end keeps its own state, but the Host folds there too, so that
    /// turning LAN access on finds the connections it already has (ADR-0012).
    pub track_connect: bool,
    /// Keep `ccrpc ls --json --watch` running from the start (the daemon). Not the desktop app: it runs a login shell,
    /// which must wait for the first ccrpc command there; its watch runs while LAN access is on.
    pub keep_ls_watch: bool,
    /// Can serve the LAN when 「局域网访问」 says so (the desktop app; the daemon has its own listeners).
    pub lan: Option<lan::LanConfig>,
    /// Download LiteLLM's price list at start (tests turn it off); the periodic refresh runs regardless.
    pub refresh_prices: bool,
}

impl HostConfig {
    /// The desktop app's directories (`dev.joy.codeck` under Caches, Application Support and Logs).
    pub fn desktop() -> Self {
        HostConfig {
            cache_dir: paths::default_cache_dir(),
            settings_path: settings::default_settings_path(),
            ccrpc_logs: None,
            images: None,
            data_root: None,
            reveal_path: None,
            open_path: None,
            track_connect: false,
            keep_ls_watch: false,
            lan: None,
            refresh_prices: true,
        }
    }
}

pub struct Host {
    /// What the methods, which only get `&self`, hand to the LAN listeners.
    me: Weak<Host>,
    state: Arc<AppState>,
    ccrpc: ccrpc::CcrpcState,
    /// With `track_connect`.
    connect: Option<Arc<connect::ConnectHub>>,
    lan: lan::LanService,
    events: EventSink,
    config: HostConfig,
}

impl Host {
    /// Opens the Engine, starts the worker (watcher, then a full scan and the text backlog) and a price
    /// refresh. One Host per process: the settings path is process-wide.
    pub fn start(config: HostConfig, events: EventSink) -> CoreResult<Arc<Host>> {
        settings::set_path(config.settings_path.clone());
        let data_root = config
            .data_root
            .clone()
            .or_else(|| settings::load().data_root.map(PathBuf::from));
        let engine = open_engine(data_root.as_deref(), config.cache_dir.clone())?;
        let (jobs, rx, backlog_cancel) = worker::channel();
        let state = Arc::new(AppState::new(engine, jobs.clone()));
        worker::spawn(
            state.clone(),
            events.clone(),
            config.cache_dir.clone(),
            rx,
            backlog_cancel,
        );
        // Starts the watcher, then FullScan -> TextBacklog.
        jobs.send(worker::Job::SetRoot);
        if config.refresh_prices {
            jobs.send(worker::Job::RefreshPrices);
        }
        let ccrpc = ccrpc::CcrpcState::new(config.ccrpc_logs.clone(), config.images.clone());
        let connect = config.track_connect.then(|| {
            let hub = connect::ConnectHub::new(events.clone(), ccrpc.writer());
            if config.keep_ls_watch {
                ccrpc.keep_watching(hub.clone(), events.clone());
            }
            hub
        });
        Ok(Arc::new_cyclic(|me| Host {
            me: me.clone(),
            state,
            ccrpc,
            connect,
            lan: lan::LanService::default(),
            events,
            config,
        }))
    }

    /// Closes every ccrpc (the process is quitting); the Sessions themselves keep running.
    pub fn shutdown(&self) {
        if let Some(hub) = &self.connect {
            hub.stop();
        }
        self.ccrpc.disconnect_all();
    }
}
