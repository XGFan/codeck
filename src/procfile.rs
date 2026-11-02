//! Process Files: `<claude config>/sessions/<pid>.json` written by every running Claude Code.

use serde::Deserialize;

use crate::locate::Roots;

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessFile {
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// `busy` / `idle` / `waiting`.
    #[serde(default)]
    pub status: Option<String>,
    /// Set while `status == "waiting"`: `permission prompt`, `input needed`, `dialog open`, …
    #[serde(default)]
    pub waiting_for: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

pub fn read(roots: &Roots, pid: u32) -> Option<ProcessFile> {
    let bytes = std::fs::read(roots.sessions_dir().join(format!("{pid}.json"))).ok()?;
    serde_json::from_slice(&bytes).ok()
}
