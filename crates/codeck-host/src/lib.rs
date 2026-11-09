//! CoDeck's Host (ADR-0011): the runtime around `codeck_core::Engine` (background worker, watcher, Live
//! polling, settings, prices, ccrpc) and the one method table both the desktop app and the daemon serve.
//! No tauri here: events go to an `EventSink`, paths come from `HostConfig`.

pub mod ccrpc;
pub mod commands;
pub mod connect;
pub mod events;
mod methods;
pub mod prices;
pub mod settings;
pub mod state;
pub mod watcher;
pub mod worker;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use codeck_core::{CoreResult, paths};

pub use crate::events::{EventSink, HostEvent};
pub use crate::methods::dispatch;
use crate::state::{AppState, open_engine};

/// The types the method table names (`for_each_method!` expands to `$crate::types::…`).
pub mod types {
    pub use crate::ccrpc::relay::{AnalyzerRecords, AnalyzerTest};
    pub use crate::ccrpc::{AnalyzerInfo, CcrpcInfo};
    pub use crate::settings::{LaunchRule, SidebarProject};
    pub use codeck_core::model::*;
}

/// Shows a path in the file manager, or opens it; `Err` is why not.
pub type PathAction = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// Where a Host keeps its state, and what it can do on the machine it runs on.
#[derive(Clone)]
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
    /// Reduce ccrpc's lines to Connect state in the Host and keep `ccrpc ls --json --watch` running (the daemon,
    /// ADR-0011): the `connect_*` / `ccrpc_ls` methods and `ConnectState` events. Off on the desktop app, whose
    /// front end does this itself.
    pub track_connect: bool,
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
        }
    }
}

pub struct Host {
    state: Arc<AppState>,
    ccrpc: ccrpc::CcrpcState,
    /// With `track_connect`.
    connect: Option<Arc<connect::ConnectHub>>,
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
        jobs.send(worker::Job::RefreshPrices);
        let ccrpc = ccrpc::CcrpcState::new(config.ccrpc_logs.clone(), config.images.clone());
        let connect = config.track_connect.then(|| {
            let hub = connect::ConnectHub::new(events.clone(), ccrpc.writer());
            ccrpc.keep_watching(hub.clone(), events.clone());
            hub
        });
        Ok(Arc::new(Host {
            state,
            ccrpc,
            connect,
            events,
            config,
        }))
    }

    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Closes every ccrpc (the process is quitting); the Sessions themselves keep running.
    pub fn shutdown(&self) {
        if let Some(hub) = &self.connect {
            hub.stop();
        }
        self.ccrpc.disconnect_all();
    }
}
