//! The application half of the IPC contract (Projects, Session lists, search, stats, diagnostics,
//! app status, events, errors), exported to `src/ipc/bindings.ts` via specta under the rules of
//! [`transcript::model`]. The parsing layer's types are re-exported from there, so
//! `codeck_core::model::…` names every IPC type.

pub mod app;
pub mod common;
pub mod diag;
pub mod events;
pub mod search;
pub mod session;
pub mod stats;

pub use ::transcript::model::*;
pub use app::*;
pub use common::*;
pub use diag::*;
pub use events::*;
pub use search::*;
pub use session::*;
pub use stats::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_code_serializes_as_exported() {
        assert_eq!(
            serde_json::to_value(ErrorCode::InvalidQuery).unwrap(),
            json!("invalidQuery")
        );
    }
}
