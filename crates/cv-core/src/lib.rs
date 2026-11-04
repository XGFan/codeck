//! Claude Viewer core: index, search and stats over a Claude Code data root, on top of the
//! `transcript` parsing crate. Strictly read-only on the data root (ADR-0002); no tauri dependency.

pub mod diag;
pub mod engine;
pub mod error;
pub mod index;
pub mod paths;
pub mod project;
pub mod scan;
pub mod search;
pub mod stats;

// The parsing layer lives in the `transcript` crate; re-exported under its old paths.
pub use transcript::{assemble, known, live, model, parse, pricing, raw};

pub use engine::{ChangeSet, Engine, EngineConfig};
pub use error::{CoreError, CoreResult};
