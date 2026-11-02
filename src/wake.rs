//! Wake-ups for the engine loop: stdin lines, Holder frames and file changes bump a counter that
//! the loop waits on, so it reacts at once instead of polling on a short timer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

#[derive(Default)]
pub struct Wake {
    m: Mutex<u64>,
    cv: Condvar,
}

impl Wake {
    pub fn new() -> Arc<Wake> {
        Arc::new(Wake::default())
    }

    pub fn bump(&self) {
        *self.m.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.cv.notify_all();
    }

    pub fn seen(&self) -> u64 {
        *self.m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits until something happened after `seen`, or `timeout` passed.
    pub fn wait(&self, seen: u64, timeout: Duration) {
        let g = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.cv.wait_timeout_while(g, timeout, |n| *n == seen);
    }
}

/// File-system watches (read-only: the OS reports changes, nothing is opened for writing).
pub struct FsWatch {
    watcher: Option<notify::RecommendedWatcher>,
    watched: Vec<(PathBuf, RecursiveMode)>,
}

impl FsWatch {
    /// Without a working watcher the engine still polls on its fallback timer.
    pub fn new(wake: Arc<Wake>) -> FsWatch {
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                wake.bump();
            }
        })
        .ok();
        FsWatch {
            watcher,
            watched: Vec::new(),
        }
    }

    /// Watches exactly `paths` (existing ones; others are picked up by a later call).
    pub fn set(&mut self, paths: &[(&Path, RecursiveMode)]) {
        let Some(w) = self.watcher.as_mut() else {
            return;
        };
        let want: Vec<(PathBuf, RecursiveMode)> = paths
            .iter()
            .filter(|(p, _)| p.exists())
            .map(|(p, m)| (p.to_path_buf(), *m))
            .collect();
        if want == self.watched {
            return;
        }
        for (p, _) in &self.watched {
            let _ = w.unwatch(p);
        }
        // Failed watches are not retried until the wanted set changes (the fallback timer covers).
        for (p, m) in &want {
            let _ = w.watch(p, *m);
        }
        self.watched = want;
    }
}
