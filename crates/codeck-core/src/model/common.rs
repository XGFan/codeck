use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

/// Project id: a git repository root (worktrees folded into the main repository), else a
/// directory's realpath; see `project::Resolver`.
pub type ProjectId = String;

/// Inclusive range of unix-epoch milliseconds; either bound may be open.
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    #[specta(type = Option<Number>)]
    pub from_ms: Option<f64>,
    #[specta(type = Option<Number>)]
    pub to_ms: Option<f64>,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    NotFound,
    InvalidQuery,
    Io,
    Index,
    Cancelled,
    Internal,
}

/// Error returned by every command. `message` is one Chinese sentence for display.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}
