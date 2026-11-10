//! What the commands share: the error line, opening the index, and text limits.

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, SecondsFormat, Utc};
use codeck_core::model::{LiveState, LiveStatus, ProjectSummary};
use codeck_core::{CoreError, Engine, EngineConfig, paths};
use codeck_host::settings;
use serde::Serialize;

/// stderr's one JSON line: `message` is one sentence for a person.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CliError {
    pub code: &'static str,
    pub message: String,
    /// The Pane left to a person, once `new` knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holder_id: Option<String>,
}

impl CliError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        CliError {
            code,
            message: message.into(),
            holder_id: None,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new("usage", message)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl From<CoreError> for CliError {
    fn from(e: CoreError) -> Self {
        let code = match e {
            CoreError::NotFound(_) => "not_found",
            CoreError::InvalidQuery(_) => "invalid_query",
            CoreError::IndexUnavailable(_) => "index_unavailable",
            CoreError::Io(_) => "io",
            CoreError::Index(_) => "index",
            CoreError::Json(_) | CoreError::Cancelled | CoreError::Internal(_) => "internal",
        };
        CliError::new(code, e.to_string())
    }
}

pub struct Context {
    pub cache_dir: Option<PathBuf>,
}

impl Context {
    /// Same resolution as the desktop app: settings, then `$CLAUDE_CONFIG_DIR`, then `~/.claude`.
    pub fn data_root(&self) -> paths::DataRoot {
        let over = settings::load().data_root.map(PathBuf::from);
        paths::resolve_data_root(over.as_deref())
    }

    pub fn engine(&self) -> Result<Engine, CliError> {
        let root = self.data_root();
        let cache_dir = self
            .cache_dir
            .clone()
            .unwrap_or_else(paths::default_cache_dir);
        Ok(Engine::open_read_only(EngineConfig {
            data_root: root.path,
            data_root_source: root.source,
            cache_dir,
        })?)
    }
}

/// `--project`: an indexed Project by its path (symlinks resolved, as Projects are identified).
pub fn find_project(engine: &Engine, path: &str) -> Result<ProjectSummary, CliError> {
    let wanted = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    engine
        .list_projects()?
        .into_iter()
        .find(|p| Path::new(&p.path) == wanted)
        .ok_or_else(|| CliError::new("not_found", format!("索引里没有这个 Project：{path}")))
}

/// Project id → path, for the output.
pub fn project_paths(engine: &Engine) -> Result<HashMap<String, String>, CliError> {
    Ok(engine
        .list_projects()?
        .into_iter()
        .map(|p| (p.id, p.path))
        .collect())
}

/// `ended` for a Session no process runs.
pub fn status_word(live: Option<&LiveState>) -> &'static str {
    match live.map(|l| l.status) {
        None => "ended",
        Some(LiveStatus::Busy) => "busy",
        Some(LiveStatus::Waiting) => "waiting",
        Some(LiveStatus::Idle) => "idle",
        Some(LiveStatus::Unknown) => "unknown",
    }
}

pub fn time_of(t: SystemTime) -> String {
    DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn time_of_ms(ms: f64) -> Option<String> {
    DateTime::from_timestamp_millis(ms as i64).map(|d| d.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// `s` cut to `max` characters, the cut marked with how long it was.
pub fn cap(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_owned();
    }
    let kept: String = s.chars().take(max).collect();
    format!("{kept}…（已截断，共 {total} 字）")
}

/// Whitespace runs (line breaks included) as one space.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Writes to stdout; a reader that went away (`| head`) is not an error.
pub fn write_stdout(text: &str) {
    let _ = std::io::stdout().write_all(text.as_bytes());
}
