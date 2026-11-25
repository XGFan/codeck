use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::{SessionId, TimeRange};

/// Days and hours are bucketed in local time (`chrono::Local`, DST-correct).
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatsRequest {
    pub time_range: Option<TimeRange>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatsOverview {
    /// Main + subagents.
    #[specta(type = Number)]
    pub output_tokens: f64,
    /// API cost in USD, main + subagents.
    #[specta(type = Number)]
    pub cost: f64,
    /// Assistant messages whose model has no price (not in `cost`).
    pub unpriced_messages: u32,
    /// Billed assistant requests carrying tokens (main + subagents).
    pub requests: u32,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DailyModelTokens {
    pub day: String,
    pub model: String,
    #[specta(type = Number)]
    pub input: f64,
    #[specta(type = Number)]
    pub output: f64,
    #[specta(type = Number)]
    pub cache_read: f64,
    #[specta(type = Number)]
    pub cache_creation: f64,
    /// API cost in USD (0 for a model without a price).
    #[specta(type = Number)]
    pub cost: f64,
    /// Billed assistant requests carrying tokens.
    pub requests: u32,
}

/// Billed requests and API cost in `[start_ms, end_ms)` (one local hour).
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HourUsage {
    #[specta(type = Number)]
    pub start_ms: f64,
    #[specta(type = Number)]
    pub end_ms: f64,
    pub requests: u32,
    /// API cost in USD (0 for unpriced models).
    #[specta(type = Number)]
    pub cost: f64,
}

/// Billed per-model tokens, cost, and requests in one local hour.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HourModelUsage {
    #[specta(type = Number)]
    pub start_ms: f64,
    pub model: String,
    #[specta(type = Number)]
    pub input: f64,
    #[specta(type = Number)]
    pub output: f64,
    #[specta(type = Number)]
    pub cache_read: f64,
    #[specta(type = Number)]
    pub cache_creation: f64,
    /// API cost in USD (0 for a model without a price).
    #[specta(type = Number)]
    pub cost: f64,
    /// Billed assistant requests carrying tokens.
    pub requests: u32,
}

/// Inclusive `[start_ms, end_ms]` activity span of a session within the queried range.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimeSpan {
    #[specta(type = Number)]
    pub start_ms: f64,
    #[specta(type = Number)]
    pub end_ms: f64,
}

/// Per-session cost, request count, and active spans within a single-day range.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub session_id: SessionId,
    pub title: String,
    /// API cost in USD (0 for unpriced models).
    #[specta(type = Number)]
    pub cost: f64,
    /// Billed assistant requests carrying tokens.
    pub requests: u32,
    pub spans: Vec<TimeSpan>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub overview: StatsOverview,
    pub daily: Vec<DailyModelTokens>,
    pub hourly: Vec<HourUsage>,
    pub hourly_models: Vec<HourModelUsage>,
    pub sessions: Vec<SessionUsage>,
}
