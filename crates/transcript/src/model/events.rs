//! Event payloads. src-tauri wraps each one in a `tauri_specta::Event` newtype.

use serde::{Deserialize, Serialize};
use specta::Type;

use super::common::{LiveState, SessionId};

/// Emitted after each debounced filesystem batch.
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionsChanged {
    pub changed: Vec<SessionId>,
    pub removed: Vec<SessionId>,
    pub projects_changed: bool,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveEntry {
    pub session_id: SessionId,
    pub state: LiveState,
}

/// Full snapshot of the Live Session set; emitted only when it changes.
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveChanged {
    pub live: Vec<LiveEntry>,
}
