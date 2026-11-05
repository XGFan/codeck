//! CoDeck core: index, search and stats over a Claude Code data root, on top of the
//! `transcript` parsing crate. Strictly read-only on the data root (ADR-0002); no tauri dependency.

pub mod diag;
pub mod engine;
pub mod error;
pub mod index;
pub mod model;
pub mod paths;
pub mod project;
pub mod scan;
pub mod search;
pub mod slash;
pub mod stats;

pub use engine::{ChangeSet, Engine, EngineConfig};
pub use error::{CoreError, CoreResult};
