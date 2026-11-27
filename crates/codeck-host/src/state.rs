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
/// override) > `$CLAUDE_CONFIG_DIR` > `~/.claude`, using the supplied `agy_root`, `haa_home` and
/// `[codex_root, pi_root]`.
pub fn open_engine(
    data_root: Option<&Path>,
    agy_root: PathBuf,
    haa_home: PathBuf,
    [codex_root, pi_root]: [PathBuf; 2],
    cache_dir: PathBuf,
    ignore_live_cwd: Option<PathBuf>,
) -> CoreResult<Engine> {
    let root = paths::resolve_data_root(data_root);
    let engine = Engine::open(EngineConfig {
        data_root: root.path,
        data_root_source: root.source,
        agy_root,
        haa_home,
        codex_root,
        pi_root,
        cache_dir,
    })?;
    Ok(engine.with_ignored_live_cwd(ignore_live_cwd))
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
