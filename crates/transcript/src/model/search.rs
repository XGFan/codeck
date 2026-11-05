use serde::{Deserialize, Serialize};
use specta::Type;

use super::common::{AgentId, BranchChoice, NodeId, SessionId, TranscriptScope};

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
