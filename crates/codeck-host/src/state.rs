//! The Engine shared by the commands and the worker.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use codeck_core::{CoreResult, Engine, EngineConfig, paths};
use parking_lot::RwLock;

use crate::worker::JobSender;

pub struct AppState {
    /// Swapped as a whole by `set_data_root`.
    pub engine: RwLock<Arc<Engine>>,
    pub jobs: JobSender,
}

/// Opens the Engine (index in `cache_dir`) on the data root resolved from `data_root` (the settings'
/// override) > `$CLAUDE_CONFIG_DIR` > `~/.claude`.
pub fn open_engine(data_root: Option<&Path>, cache_dir: PathBuf) -> CoreResult<Engine> {
    let root = paths::resolve_data_root(data_root);
    Engine::open(EngineConfig {
        data_root: root.path,
        data_root_source: root.source,
        cache_dir,
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
