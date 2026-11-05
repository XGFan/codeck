use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

/// Session ID as written by Claude Code (a UUID string).
pub type SessionId = String;
/// Display node id: the entry uuid (for a merged assistant node, the uuid of its first fragment in file order).
pub type NodeId = String;
/// Subagent id: the agent file stem without the `agent-` prefix.
pub type AgentId = String;
/// Project id: a git repository root (worktrees folded into the main repository), else a
/// directory's realpath; see `project::Resolver`.
pub type ProjectId = String;

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    #[specta(type = Number)]
    pub input: f64,
    #[specta(type = Number)]
    pub output: f64,
    #[specta(type = Number)]
    pub cache_read: f64,
    #[specta(type = Number)]
    pub cache_creation: f64,
}

impl TokenTotals {
    /// Field-wise max: the fragments of one `message.id` each repeat (part of) its usage.
    pub fn max_with(&mut self, u: &TokenTotals) {
        self.input = self.input.max(u.input);
        self.output = self.output.max(u.output);
        self.cache_read = self.cache_read.max(u.cache_read);
        self.cache_creation = self.cache_creation.max(u.cache_creation);
    }

    pub fn add(&mut self, u: &TokenTotals) {
        self.input += u.input;
        self.output += u.output;
        self.cache_read += u.cache_read;
        self.cache_creation += u.cache_creation;
    }
}

/// Inclusive range of unix-epoch milliseconds; either bound may be open.
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimeRange {
    #[specta(type = Option<Number>)]
    pub from_ms: Option<f64>,
    #[specta(type = Option<Number>)]
    pub to_ms: Option<f64>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TranscriptScope {
    Main,
    Subagent { agent_id: AgentId },
}

/// Selects a non-default Branch: at `anchor_key`, follow `head_id` (an id, not an index, so it is stable under live appends).
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BranchChoice {
    pub anchor_key: String,
    pub head_id: NodeId,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LiveStatus {
    Busy,
    Waiting,
    Idle,
    Unknown,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveState {
    pub status: LiveStatus,
    pub raw_status: Option<String>,
    /// Only set while Waiting (`permission prompt`, `input needed`, ...).
    pub waiting_for: Option<String>,
    pub pid: Option<u32>,
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
