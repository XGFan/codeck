//! Analysis records: one file per analysis the analyzer ran, learned or failed, with what it was
//! shown and what it answered, for looking into a wrong or missing layout later. They live in the
//! local data directory (0600 files, as the layout library: screens and tool inputs are in them);
//! only the newest [`MAX_RECORDS`] are kept.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::layout::LayoutKind;

pub const MAX_RECORDS: usize = 200;

/// `CCRPC_ANALYSIS_DIR`, else `$XDG_DATA_HOME/ccrpc/analyses`, else `~/.local/share/ccrpc/analyses`.
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CCRPC_ANALYSIS_DIR") {
        return PathBuf::from(d);
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data.join("ccrpc").join("analyses")
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The layout passed its checks and went into the library.
    Learned,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// When the answer came back (UTC, RFC 3339).
    pub at: String,
    pub session_id: Option<String>,
    pub claude_version: Option<String>,
    pub model: String,
    pub base_url: String,
    pub elapsed_ms: u64,
    /// The Process File's `waitingFor`.
    pub waiting_for: String,
    /// The pending tool calls sent along: `{"tool", "input"}`.
    pub pending: Vec<Value>,
    /// The screen sent.
    pub screen: Vec<String>,
    /// The model's text, when a response came back.
    pub reply: Option<String>,
    pub outcome: Outcome,
    /// The layout the answer made (also when it failed its checks on the screen).
    pub layout: Option<String>,
    pub kind: Option<LayoutKind>,
    pub title: Option<String>,
    /// Where a learned layout was saved.
    pub path: Option<String>,
    pub error: Option<String>,
}

/// Writes `r` into `dir` (created 0700) and drops the oldest records beyond [`MAX_RECORDS`].
pub fn write(dir: &Path, r: &Record) -> Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("建不了分析记录目录 {}", dir.display()))?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // Millisecond file names sort in time order; the pid keeps two ccrpc processes apart.
    let path = dir.join(format!("{ms:013}-{}.json", std::process::id()));
    let json = serde_json::to_string_pretty(r)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut f| f.write_all((json + "\n").as_bytes()))
        .with_context(|| format!("写不了分析记录 {}", path.display()))?;
    let files = files(dir);
    for old in files.iter().skip(MAX_RECORDS) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// Record files in `dir`, newest first.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    v.reverse();
    v
}

/// The records in `dir`, newest first, with their files; unreadable files are skipped.
pub fn list(dir: &Path) -> Vec<(PathBuf, Record)> {
    files(dir)
        .into_iter()
        .filter_map(|p| {
            let r = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((p, r))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(error: Option<&str>) -> Record {
        Record {
            at: "2026-11-06T03:38:30Z".into(),
            session_id: Some("s".into()),
            claude_version: Some("2.1.318".into()),
            model: "m".into(),
            base_url: "http://127.0.0.1".into(),
            elapsed_ms: 1200,
            waiting_for: "dialog open".into(),
            pending: Vec::new(),
            screen: vec!["Rewind".into()],
            reply: Some(r#"{"kind":"none"}"#.into()),
            outcome: if error.is_some() {
                Outcome::Failed
            } else {
                Outcome::Learned
            },
            layout: None,
            kind: None,
            title: None,
            path: None,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn records_come_back_newest_first_and_only_the_newest_are_kept() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..MAX_RECORDS + 3 {
            // Older records from before: file names in time order.
            std::fs::write(
                d.path().join(format!("{:013}-1.json", 1_000 + i)),
                serde_json::to_vec(&record(None)).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(d.path().join("notes.txt"), "x").unwrap();
        write(
            d.path(),
            &record(Some("分析器认为屏幕上没有等待作答的菜单")),
        )
        .unwrap();
        let got = list(d.path());
        assert_eq!(got.len(), MAX_RECORDS);
        assert_eq!(got[0].1.outcome, Outcome::Failed);
        // The oldest ones went.
        assert!(!d.path().join(format!("{:013}-1.json", 1_000)).exists());
        assert!(d.path().join("notes.txt").exists());
    }
}
