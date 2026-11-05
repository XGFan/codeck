//! Reading Claude Code's on-disk data: tolerant transcript parsing (`raw`, `parse`), assembly into
//! the display model (`assemble`, `model`), process files (`live`) and pricing. Strictly read-only
//! on the data root (files are only opened read-only); no SQLite, no tauri. Used by
//! codeck-core (index, search, stats) and by ccrpc (following a running Session).

pub mod assemble;
pub mod error;
pub mod known;
pub mod layout;
pub mod live;
pub mod model;
pub mod parse;
pub mod pricing;
pub mod raw;

pub use error::{TranscriptError, TranscriptResult};
