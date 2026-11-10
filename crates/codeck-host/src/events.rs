//! What the Host reports. The desktop app turns each one into its typed tauri event; clients on the LAN get
//! `{event: name(), payload: payload()}`, the same names and payloads. `ConnectState` / `ConnectRemoved`
//! come only from a Host that tracks Connect state, for its remote clients; they have no tauri event.

use std::sync::Arc;

use codeck_core::model::{IndexStatus, LiveChanged, SessionsChanged};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub enum HostEvent {
    /// Index progress; throttled to at most 5/s.
    IndexStatus(IndexStatus),
    /// After each debounced filesystem batch.
    SessionsChanged(SessionsChanged),
    /// Only when the Live Session set or a status changes.
    LiveChanged(LiveChanged),
    /// A newly fetched price list changed the API cost of indexed messages.
    PricesChanged,
    /// One line of a connection's ccrpc stdout, as it was written (not parsed).
    CcrpcLine { conn: String, line: String },
    /// A connection's ccrpc exited; `stderr` is its tail (at most 4000 characters).
    CcrpcExit {
        conn: String,
        code: Option<i32>,
        stderr: String,
    },
    /// `ccrpc ls --json --watch` printed the list again: its JSON array as printed.
    CcrpcLs { ls: String },
    /// `ccrpc ls --json --watch` ended by itself; `message` is why.
    CcrpcLsEnded { message: String },
    /// The watch the Host keeps ended by itself: remote clients get it as `CcrpcLsEnded`; the desktop's window does
    /// not, since it would stop listening and the Host starts the watch again by itself.
    CcrpcLsKeptEnded { message: String },
    /// A connection's Connect state changed (only with `HostConfig::track_connect`; the whole state).
    ConnectState(Box<crate::connect::ConnState>),
    /// A connection's ccrpc exited and its state is gone (after its last `ConnectState`, with `procExit`).
    ConnectRemoved { conn: String },
    /// A remote client started or took up this connection: the desktop's front end adopts it (ADR-0012).
    CcrpcRemoteConn {
        conn: String,
        holder_id: Option<String>,
    },
    /// This connection is being closed on purpose (by a remote client, or replaced): its exit is no failure.
    CcrpcClosing { conn: String },
}

pub type EventSink = Arc<dyn Fn(HostEvent) + Send + Sync>;

impl HostEvent {
    /// The event's name, as tauri-specta names the desktop app's typed events.
    pub fn name(&self) -> &'static str {
        match self {
            HostEvent::IndexStatus(_) => "index-status-event",
            HostEvent::SessionsChanged(_) => "sessions-changed-event",
            HostEvent::LiveChanged(_) => "live-changed-event",
            HostEvent::PricesChanged => "prices-changed-event",
            HostEvent::CcrpcLine { .. } => "ccrpc-line-event",
            HostEvent::CcrpcExit { .. } => "ccrpc-exit-event",
            HostEvent::CcrpcLs { .. } => "ccrpc-ls-event",
            HostEvent::CcrpcLsEnded { .. } | HostEvent::CcrpcLsKeptEnded { .. } => {
                "ccrpc-ls-ended-event"
            }
            HostEvent::ConnectState(_) => "connect-state-event",
            HostEvent::ConnectRemoved { .. } => "connect-removed-event",
            HostEvent::CcrpcRemoteConn { .. } => "ccrpc-remote-conn-event",
            HostEvent::CcrpcClosing { .. } => "ccrpc-closing-event",
        }
    }

    /// The payload as the desktop app's event serializes it (a newtype event as its inner value).
    pub fn payload(&self) -> Value {
        let v = match self {
            HostEvent::IndexStatus(s) => serde_json::to_value(s),
            HostEvent::SessionsChanged(s) => serde_json::to_value(s),
            HostEvent::LiveChanged(l) => serde_json::to_value(l),
            HostEvent::PricesChanged => Ok(Value::Null),
            HostEvent::CcrpcLine { conn, line } => Ok(json!({ "conn": conn, "line": line })),
            HostEvent::CcrpcExit { conn, code, stderr } => {
                Ok(json!({ "conn": conn, "code": code, "stderr": stderr }))
            }
            HostEvent::CcrpcLs { ls } => Ok(json!({ "ls": ls })),
            HostEvent::CcrpcLsEnded { message } | HostEvent::CcrpcLsKeptEnded { message } => {
                Ok(json!({ "message": message }))
            }
            HostEvent::ConnectState(s) => serde_json::to_value(s),
            HostEvent::ConnectRemoved { conn } | HostEvent::CcrpcClosing { conn } => {
                Ok(json!({ "conn": conn }))
            }
            HostEvent::CcrpcRemoteConn { conn, holder_id } => {
                Ok(json!({ "conn": conn, "holderId": holder_id }))
            }
        };
        v.unwrap_or(Value::Null)
    }
}
