//! CoDeck core: index, search and stats over a Claude Code data root, on top of the
//! `transcript` parsing crate. Strictly read-only on the data root (ADR-0002); no tauri dependency.

pub mod diag;
pub mod engine;
pub mod error;
pub mod index;
pub mod paths;
pub mod project;
pub mod scan;
pub mod search;
pub mod slash;
pub mod stats;

// IPC types of the parsing layer, re-exported so src-tauri keeps `codeck_core::model::…`; the
// rest of the `transcript` crate is used by its own paths.
pub use transcript::model;

pub use engine::{ChangeSet, Engine, EngineConfig};
pub use error::{CoreError, CoreResult};
