use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::common::{LiveState, NodeId, ProjectId, SessionId, TimeRange, TokenTotals};

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: ProjectId,
    pub path: String,
    pub display_name: String,
    /// The project directory no longer exists on disk.
    pub missing: bool,
    pub session_count: u32,
    #[specta(type = Number)]
    pub last_active_ms: f64,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionSort {
    #[default]
    LastActive,
    Created,
    Messages,
    /// Sorts by output tokens (main + subagents).
    Tokens,
    /// Sorts by API cost (main + subagents).
    Cost,
}

/// Stats drill-down filter; days are local calendar days.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Drill {
    /// `day` is `YYYY-MM-DD`.
    Day {
        day: String,
    },
    Model {
        model: String,
    },
    /// Main-file messages in `[from_ms, to_ms)` (an activity bar).
    Span {
        #[specta(type = Number)]
        from_ms: f64,
        #[specta(type = Number)]
        to_ms: f64,
    },
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuery {
    /// Sessions started in or editing any of these Projects; empty = all.
    pub project_ids: Vec<ProjectId>,
    pub sort: SessionSort,
    pub descending: bool,
    pub time_range: Option<TimeRange>,
    pub drill: Option<Drill>,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TitleSource {
    Custom,
    Ai,
    FirstPrompt,
    Untitled,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ForkOrigin {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub fork_point_id: Option<NodeId>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub title: String,
    pub title_source: TitleSource,
    #[specta(type = Number)]
    pub created_ms: f64,
    #[specta(type = Number)]
    pub last_active_ms: f64,
    pub message_count: u32,
    pub tool_call_count: u32,
    pub subagent_count: u32,
    /// Main + all subagents.
    pub tokens: TokenTotals,
    /// API cost in USD, main + all subagents (see `pricing`); unpriced messages count as 0.
    #[specta(type = Number)]
    pub cost: f64,
    /// Assistant messages whose model has no price.
    pub unpriced_messages: u32,
    pub git_branch: Option<String>,
    pub live: Option<LiveState>,
    pub fork_origin: Option<ForkOrigin>,
    pub primary_model: Option<String>,
    /// Projects whose files an edit tool (main or Subagent) wrote, `project_id` included if edited.
    pub edited_project_ids: Vec<ProjectId>,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileRole {
    Main,
    Subagent,
    WorkflowSubagent,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    pub path: String,
    #[specta(type = Number)]
    pub size: f64,
    pub role: FileRole,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ForkChild {
    pub session_id: SessionId,
    pub title: String,
    #[specta(type = Number)]
    pub created_ms: f64,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub cwd: Option<String>,
    pub project_path: String,
    pub project_missing: bool,
    #[specta(type = Number)]
    pub duration_ms: f64,
    pub models: Vec<String>,
    pub tokens_main: TokenTotals,
    pub tokens_subagents: TokenTotals,
    /// Cost per model (main + subagents), most expensive first.
    pub model_costs: Vec<ModelCost>,
    pub versions: Vec<String>,
    pub files: Vec<SourceFile>,
    pub forks: Vec<ForkChild>,
    pub failed_lines: u32,
    /// `claude --resume <id>`.
    pub resume_command: String,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    pub model: String,
    #[specta(type = Number)]
    pub cost: f64,
    /// Messages of this model without a price (all of them, or none).
    pub unpriced_messages: u32,
}
