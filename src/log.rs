//! `--log FILE`: what ccrpc decided and did (state, screen recognition, hook events, keys), for
//! looking into `unrecognized` and timeouts. Off unless asked for.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

/// Appends to `path` (0600) from now on.
pub fn open(path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| anyhow::anyhow!("打不开日志 {}：{e}", path.display()))?;
    let _ = LOG.set(Mutex::new(f));
    Ok(())
}

pub fn enabled() -> bool {
    LOG.get().is_some()
}

pub fn line(args: std::fmt::Arguments) {
    let Some(m) = LOG.get() else { return };
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    if let Ok(mut f) = m.lock() {
        let _ = writeln!(f, "{t:.3} {args}");
    }
}

/// `clog!("…", …)`: one log line when `--log` is on.
#[macro_export]
macro_rules! clog {
    ($($t:tt)*) => {
        if $crate::log::enabled() {
            $crate::log::line(format_args!($($t)*))
        }
    };
}
