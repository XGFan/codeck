use serde::{Deserialize, Serialize};
use specta::Type;

use super::common::{NodeId, SessionId};

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

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileRole {
    Main,
    Subagent,
    WorkflowSubagent,
}
