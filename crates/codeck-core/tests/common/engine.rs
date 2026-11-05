//! Engine setup for the codeck-core integration tests (the `transcript` tests do not include
//! this: it depends on codeck-core).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use codeck_core::model::DataRootSource;
use codeck_core::{Engine, EngineConfig};
use rusqlite::Connection;
use tempfile::TempDir;

use super::{copy_dir_all, fixture_dir, fixture_root};

/// An engine over the data root `root` with its index under `cache`.
pub fn open_engine(root: &Path, cache: &Path) -> Engine {
    Engine::open(EngineConfig {
        data_root: root.to_path_buf(),
        data_root_source: DataRootSource::Settings,
        cache_dir: cache.to_path_buf(),
    })
    .expect("open engine")
}

/// A temp data root holding the given scenarios: one scenario via [`fixture_root`] (with its
/// extras for `copies` / `live`), several merged by copying.
pub fn data_root(scenarios: &[&str]) -> TempDir {
    match scenarios {
        [one] => fixture_root(one),
        many => {
            let tmp = tempfile::tempdir().unwrap();
            for s in many {
                copy_dir_all(&fixture_dir(s), tmp.path()).unwrap();
            }
            tmp
        }
    }
}

/// A data root, the cache dir holding its index, and the engine over both.
pub struct Env {
    pub root: TempDir,
    pub cache: TempDir,
    pub engine: Engine,
}

impl Env {
    /// An engine over `root` with a fresh cache dir, after a full scan.
    pub fn scanned(root: TempDir) -> Env {
        let cache = tempfile::tempdir().unwrap();
        let engine = open_engine(root.path(), cache.path());
        engine.scan_all(&|_| {}).expect("scan_all");
        Env {
            root,
            cache,
            engine,
        }
    }

    /// [`Env::scanned`], then the full-text backlog indexed to the end.
    pub fn indexed(root: TempDir) -> Env {
        let env = Env::scanned(root);
        env.engine
            .index_text_backlog(&|_| {}, &AtomicBool::new(false))
            .unwrap();
        env
    }

    pub fn db(&self) -> Connection {
        Connection::open(self.cache.path().join("index.sqlite")).unwrap()
    }
}
