use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

/// Kind of a subscription rate-limit bucket returned by Claude Code's `get_usage`.
#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlanLimitKind {
    /// Rolling 5-hour session window.
    Session,
    /// Rolling 7-day window across all models.
    WeeklyAll,
    /// Rolling 7-day window scoped to a specific model (named by `PlanLimit::model`).
    WeeklyScoped,
}

/// One rate-limit item of the logged-in account's subscription.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanLimit {
    pub kind: PlanLimitKind,
    /// Model display name for `weeklyScoped` (e.g. `"Fable"`), `None` otherwise.
    pub model: Option<String>,
    /// Percentage used (finite, clamped at `>= 0`).
    #[specta(type = Number)]
    pub percent: f64,
    /// Next reset timestamp in unix-epoch milliseconds, when provided and parseable.
    #[specta(type = Option<Number>)]
    pub resets_ms: Option<f64>,
}

/// Snapshot of the logged-in account's subscription usage.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanUsage {
    /// Subscription plan name as reported by Claude Code (e.g. `"max"`, `"pro"`).
    pub plan: Option<String>,
    /// When this snapshot was fetched (unix-epoch milliseconds).
    #[specta(type = Number)]
    pub fetched_ms: f64,
    pub limits: Vec<PlanLimit>,
}
