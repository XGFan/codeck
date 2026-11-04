//! Typed events (tauri-specta). Payload types live in `codeck_core::model::events`.

use codeck_core::model::{IndexStatus, LiveChanged, SessionsChanged};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

/// Index progress; throttled to at most 5/s.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct IndexStatusEvent(pub IndexStatus);

/// After each debounced filesystem batch.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct SessionsChangedEvent(pub SessionsChanged);

/// A newly fetched price list changed the API cost of indexed messages.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct PricesChangedEvent;

/// Only when the Live Session set or a status changes.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct LiveChangedEvent(pub LiveChanged);

/// One line of a connection's ccrpc stdout, as it was written (not parsed).
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct CcrpcLineEvent {
    pub conn: String,
    pub line: String,
}

/// A connection's ccrpc exited; `stderr` is its tail (at most 4000 characters).
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct CcrpcExitEvent {
    pub conn: String,
    pub code: Option<i32>,
    pub stderr: String,
}
