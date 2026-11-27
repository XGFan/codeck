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

/// `ccrpc ls --json --watch` printed the list again: its JSON array as printed (the front end parses it).
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct CcrpcLsEvent {
    pub ls: String,
}

/// `ccrpc ls --json --watch` ended by itself; `message` is why.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct CcrpcLsEndedEvent {
    pub message: String,
}

/// A connection's ccrpc exited; `stderr` is its tail (at most 4000 characters).
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct CcrpcExitEvent {
    pub conn: String,
    pub code: Option<i32>,
    pub stderr: String,
}

/// A client on the LAN started or took up this connection: the window adopts it.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CcrpcRemoteConnEvent {
    pub conn: String,
    pub holder_id: Option<String>,
}

/// This connection is being closed on purpose (by a client on the LAN, or replaced): its exit is no failure.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct CcrpcClosingEvent {
    pub conn: String,
}

/// `haa ls --all --json --watch` (instance `id`) printed the Worker list again: its JSON array as printed.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct HaaWorkersEvent {
    pub id: u32,
    pub workers: String,
}

/// The Host starts (`active`) or ends refreshing a Session's prompt cache (a `/btw` of its own); `error` is why the
/// refresh failed. Meanwhile the Session's Live state says waiting.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KeepAliveEvent {
    pub session_id: String,
    pub active: bool,
    pub error: Option<String>,
}

/// `haa ls --all --json --watch` (instance `id`) ended by itself; `message` is why.
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct HaaWatchEndedEvent {
    pub id: u32,
    pub message: String,
}

/// A system notification was clicked: the `url` it was sent with (`commands::notify`).
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
pub struct NotificationClickEvent {
    pub url: String,
}

/// The paired LAN device list changed (a device paired or was removed).
#[derive(Serialize, Deserialize, Type, Event, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LanDevicesEvent {
    pub devices: Vec<codeck_host::lan::PairedDevice>,
    pub paired_offer: Option<String>,
    pub paired_device: Option<String>,
}
