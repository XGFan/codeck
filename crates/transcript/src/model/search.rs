use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::common::{AgentId, BranchChoice, NodeId, ProjectId, SessionId, TranscriptScope};
use super::session::SessionSummary;

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SearchRole {
    User,
    Assistant,
    ToolInput,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    /// Whitespace-separated AND terms, `"phrase"`, `-exclude`.
    pub query: String,
    /// Sessions started in or editing this Project.
    pub project_id: Option<ProjectId>,
    /// Default 50.
    pub max_sessions: u32,
    /// Default 5.
    pub hits_per_session: u32,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SearchMode {
    Fts,
    Like,
    Mixed,
    Scan,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnippetPart {
    pub text: String,
    pub hit: bool,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub node_id: NodeId,
    pub agent_id: Option<AgentId>,
    /// `agentType` of the Subagent Run holding the hit (from its `.meta.json`).
    pub agent_type: Option<String>,
    pub tool_use_id: Option<String>,
    pub role: SearchRole,
    #[specta(type = Option<Number>)]
    pub timestamp_ms: Option<f64>,
    pub on_main_line: bool,
    pub snippet: Vec<SnippetPart>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchGroup {
    pub session: SessionSummary,
    pub hit_count: u32,
    pub hits: Vec<SearchHit>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub groups: Vec<SearchGroup>,
    pub total_sessions: u32,
    pub total_hits: u32,
    pub mode: SearchMode,
    #[specta(type = Number)]
    pub elapsed_ms: f64,
    /// False while the full-text backlog is still being indexed.
    pub index_complete: bool,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JumpRequest {
    pub session_id: SessionId,
    /// Any entry uuid (fragment or absorbed tool_result uuids are mapped to the display node).
    pub node_id: NodeId,
    pub agent_id: Option<AgentId>,
    pub tool_use_id: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JumpTarget {
    pub session_id: SessionId,
    pub scope: TranscriptScope,
    pub branch_choices: Vec<BranchChoice>,
    /// Display node id.
    pub node_id: NodeId,
    pub tool_use_id: Option<String>,
    /// Outermost → innermost Subagent Run ids leading to the target.
    pub agent_path: Vec<AgentId>,
    pub in_subagent: bool,
    pub in_abandoned_branch: bool,
    pub needs_hidden: bool,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FindRequest {
    pub session_id: SessionId,
    pub scope: TranscriptScope,
    pub branch_choices: Vec<BranchChoice>,
    pub include_hidden: bool,
    pub query: String,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FindLocation {
    Text,
    Thinking,
    ToolInput { tool_use_id: String },
    ToolOutput { tool_use_id: String },
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FindMatch {
    pub node_id: NodeId,
    pub location: FindLocation,
    pub count: u32,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FindResult {
    pub matches: Vec<FindMatch>,
    pub total: u32,
}
