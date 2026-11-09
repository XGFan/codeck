//! Fetches LiteLLM's price list for API cost (ADR-0005); the engine keeps only `claude-*` rows.

use std::time::{Duration, SystemTime};

const URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const TIMEOUT: Duration = Duration::from_secs(20);
/// A cached list younger than this is not fetched again.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub fn is_fresh(fetched_at: Option<SystemTime>) -> bool {
    fetched_at
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < MAX_AGE)
}

pub fn fetch() -> Result<String, ureq::Error> {
    ureq::get(URL)
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .call()?
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_string()
}
