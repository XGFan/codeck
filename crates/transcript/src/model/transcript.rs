use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::common::{AgentId, BranchChoice, NodeId, SessionId, TokenTotals, TranscriptScope};

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptRequest {
    pub session_id: SessionId,
    pub scope: TranscriptScope,
    pub branch_choices: Vec<BranchChoice>,
    pub include_hidden: bool,
}

/// The whole selected path of a Session (or Subagent Run) with capped content.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub session_id: SessionId,
    pub scope: TranscriptScope,
    /// Changes whenever any source file of the Session changes.
    #[specta(type = Number)]
    pub revision: f64,
    pub nodes: Vec<Node>,
    pub branch_points: Vec<BranchPoint>,
    pub hidden_count: u32,
    pub inherited: Option<InheritedRange>,
    pub subagents: Vec<SubagentRun>,
    pub workflows: Vec<WorkflowRun>,
    pub orphan_subagent_ids: Vec<AgentId>,
}

/// Fork-inherited prefix of the transcript.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InheritedRange {
    pub origin_session_id: SessionId,
    pub origin_title: Option<String>,
    pub last_inherited_id: NodeId,
    pub count: u32,
}

/// UI renders `‹ i/n ›` before the node `selected_head_id`.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BranchPoint {
    pub anchor_key: String,
    pub selected_head_id: NodeId,
    pub options: Vec<BranchOption>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BranchOption {
    pub head_id: NodeId,
    pub preview: String,
    #[specta(type = Option<Number>)]
    pub timestamp_ms: Option<f64>,
    pub reply_count: u32,
    pub is_main_line: bool,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: NodeId,
    #[specta(type = Option<Number>)]
    pub timestamp_ms: Option<f64>,
    pub hidden: bool,
    pub inherited: bool,
    pub body: NodeBody,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NodeBody {
    UserPrompt {
        text: String,
        images: Vec<ImageRef>,
        origin: PromptOrigin,
    },
    Assistant {
        message_id: Option<String>,
        model: Option<String>,
        blocks: Vec<AssistantBlock>,
        usage: Option<TokenTotals>,
        is_api_error: bool,
    },
    CompactBoundary {
        trigger: Option<String>,
        #[specta(type = Option<Number>)]
        pre_tokens: Option<f64>,
    },
    CompactSummary {
        text: String,
    },
    System {
        subtype: String,
        level: Option<String>,
        text: String,
    },
    Attachment {
        attachment_type: String,
        text: String,
    },
    Unknown {
        entry_type: String,
        raw_json: String,
    },
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PromptOrigin {
    Human,
    /// `<command-name>name</command-name>…<command-args>args</command-args>`.
    Command {
        name: String,
        args: String,
    },
    CommandOutput,
    /// Background agent completion (`<task-notification>`).
    TaskNotification {
        task_id: Option<String>,
        tool_use_id: Option<String>,
        status: Option<String>,
        summary: Option<String>,
    },
    /// A message from another Claude session of an agent team
    /// (`<teammate-message teammate_id="…" color="…" summary="…">`); not typed by the user.
    Teammate {
        teammate_id: Option<String>,
        color: Option<String>,
        summary: Option<String>,
    },
    Meta,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
// Tool calls are the most common block, so boxing them would not save memory.
#[allow(clippy::large_enum_variant)]
pub enum AssistantBlock {
    Text {
        text: String,
    },
    /// Omitted when empty.
    Thinking {
        text: String,
    },
    ToolCall(ToolCall),
    Unknown {
        block_type: String,
        raw_json: String,
    },
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool_use_id: String,
    pub name: String,
    /// Tool input with string values capped at 4 KB each.
    pub input_json: String,
    pub input_truncated: bool,
    pub result: Option<ToolResult>,
    pub subagent_id: Option<AgentId>,
    pub workflow_run_id: Option<String>,
    pub notification_node_id: Option<NodeId>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub is_error: bool,
    #[specta(type = Option<Number>)]
    pub timestamp_ms: Option<f64>,
    /// Capped at 2 KB / 40 lines.
    pub text: String,
    pub images: Vec<ImageRef>,
    #[specta(type = Number)]
    pub total_bytes: f64,
    pub truncated: bool,
    pub persisted: Option<PersistedRef>,
    /// Capped (≤ 8 KB, whole fields dropped) subset of `toolUseResult`:
    /// Bash {stdout,stderr,interrupted,returnCodeInterpretation,backgroundTaskId},
    /// AskUserQuestion {questions,answers,annotations}, Task* {task,statusChange}, Edit {structuredPatch?}.
    pub extra_json: Option<String>,
}

/// Output offloaded to `<session dir>/tool-results/<file_name>`.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedRef {
    pub file_name: String,
    #[specta(type = Number)]
    pub size_bytes: f64,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    pub node_id: NodeId,
    pub ordinal: u32,
    pub media_type: String,
    #[specta(type = Number)]
    pub bytes: f64,
    pub tool_use_id: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRun {
    pub agent_id: AgentId,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub name: Option<String>,
    pub parent_agent_id: Option<AgentId>,
    pub spawn_depth: u32,
    pub tool_use_id: Option<String>,
    pub workflow_run_id: Option<String>,
    pub is_async: bool,
    pub status: Option<String>,
    pub model: Option<String>,
    pub message_count: u32,
    pub tool_call_count: u32,
    pub tokens: TokenTotals,
    #[specta(type = Option<Number>)]
    pub started_ms: Option<f64>,
    #[specta(type = Option<Number>)]
    pub ended_ms: Option<f64>,
    pub prompt_preview: Option<String>,
    /// Agent tool_result text, else the task-notification summary.
    pub final_text: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    pub index: u32,
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAgent {
    pub agent_id: AgentId,
    pub label: Option<String>,
    pub phase_index: Option<u32>,
    pub state: Option<String>,
    pub model: Option<String>,
    #[specta(type = Number)]
    pub tokens: f64,
    pub tool_calls: u32,
    #[specta(type = Number)]
    pub duration_ms: f64,
    pub result_preview: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub run_id: String,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub status: Option<String>,
    pub tool_use_id: Option<String>,
    pub phases: Vec<WorkflowPhase>,
    pub agents: Vec<WorkflowAgent>,
    #[specta(type = Option<Number>)]
    pub duration_ms: Option<f64>,
    #[specta(type = Option<Number>)]
    pub total_tokens: Option<f64>,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToolPart {
    Input,
    Output,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetailRequest {
    pub session_id: SessionId,
    pub scope: TranscriptScope,
    pub tool_use_id: String,
    pub part: ToolPart,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DetailSource {
    Inline,
    PersistedFile,
    Missing,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetail {
    pub text: String,
    /// Capped at 8 MB.
    pub truncated: bool,
    #[specta(type = Number)]
    pub total_bytes: f64,
    pub source: DetailSource,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageRequest {
    pub session_id: SessionId,
    pub scope: TranscriptScope,
    pub image: ImageRef,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageData {
    pub media_type: String,
    pub data_base64: String,
}
