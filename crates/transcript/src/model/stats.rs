use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::common::{SessionId, TimeRange};

/// Days and week-hours are bucketed in local time (`chrono::Local`, DST-correct).
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatsRequest {
    pub time_range: Option<TimeRange>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatsOverview {
    pub sessions: u32,
    /// Main-file messages only (same definition as `message_count`).
    pub messages: u32,
    /// Main + subagents.
    #[specta(type = Number)]
    pub output_tokens: f64,
    pub active_days: u32,
    /// API cost in USD, main + subagents.
    #[specta(type = Number)]
    pub cost: f64,
    /// Assistant messages whose model has no price (not in `cost`).
    pub unpriced_messages: u32,
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
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DayCount {
    pub day: String,
    pub messages: u32,
}

/// Width of one activity bar; aligned in local time (whole n hours, midnight, Monday, the 1st).
#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivityBucket {
    #[default]
    Hour1,
    Hour3,
    Hour6,
    Hour12,
    Day,
    Week,
    Month,
}

/// Main-file messages in `[start_ms, end_ms)`.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityBar {
    #[specta(type = Number)]
    pub start_ms: f64,
    #[specta(type = Number)]
    pub end_ms: f64,
    pub messages: u32,
}

/// Message histogram over the requested range: the finest bucket with at most
/// `codeck_core::stats::MAX_ACTIVITY_BARS` bars; empty bars included.
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub bucket: ActivityBucket,
    pub bars: Vec<ActivityBar>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionStat {
    pub session_id: SessionId,
    pub title: String,
    /// Main-file messages in range.
    pub messages: u32,
    #[specta(type = Number)]
    pub output_tokens: f64,
    #[specta(type = Number)]
    pub cost: f64,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub overview: StatsOverview,
    pub daily: Vec<DailyModelTokens>,
    pub heat_daily: Vec<DayCount>,
    pub activity: Activity,
    /// The most expensive sessions in range (at most `codeck_core::stats::TOP_SESSIONS`).
    pub sessions: Vec<SessionStat>,
}
