//! Search: query parsing and FTS5 trigram / LIKE search.

pub mod fts;
pub mod query;
pub mod snippet;

pub use fts::run_fts;
pub use query::{ParsedQuery, parse_query};
