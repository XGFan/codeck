use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::TimeRange;

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

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub overview: StatsOverview,
    pub daily: Vec<DailyModelTokens>,
    pub hourly: Vec<HourUsage>,
}
