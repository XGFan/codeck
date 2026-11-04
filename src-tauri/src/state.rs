//! Shared app state managed by tauri.

use std::path::Path;
use std::sync::Arc;

use codeck_core::{CoreResult, Engine, EngineConfig, paths};
use parking_lot::RwLock;

use crate::settings::Settings;
use crate::worker::JobSender;

pub struct AppState {
    /// Swapped as a whole by `set_data_root`.
    pub engine: RwLock<Arc<Engine>>,
    pub jobs: JobSender,
}

/// Opens the Engine on the data root resolved from `settings` > `$CLAUDE_CONFIG_DIR` > `~/.claude`.
pub fn open_engine(settings: &Settings) -> CoreResult<Engine> {
    let root = paths::resolve_data_root(settings.data_root.as_deref().map(Path::new));
    Engine::open(EngineConfig {
        data_root: root.path,
        data_root_source: root.source,
        cache_dir: paths::default_cache_dir(),
    })
}

impl AppState {
    pub fn new(engine: Engine, jobs: JobSender) -> Self {
        AppState {
            engine: RwLock::new(Arc::new(engine)),
            jobs,
        }
    }

    pub fn engine(&self) -> Arc<Engine> {
        self.engine.read().clone()
    }
}
