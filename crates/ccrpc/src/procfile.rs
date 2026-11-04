//! Process Files: `<claude config>/sessions/<pid>.json` written by every running Claude Code.
//! The struct and the liveness check (pid plus `procStart`) are the transcript crate's.

pub use ::transcript::live::{ProcessFile, is_alive};

use crate::locate::Roots;

pub fn read(roots: &Roots, pid: u32) -> Option<ProcessFile> {
    let bytes = std::fs::read(roots.sessions_dir().join(format!("{pid}.json"))).ok()?;
    serde_json::from_slice(&bytes).ok()
}
