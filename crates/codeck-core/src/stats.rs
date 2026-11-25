//! Stats panel aggregation (local-time buckets: daily per-model totals and hourly usage).
//!
//! Semantics (each drillable dimension has a [`Drill`](crate::model::Drill) in `index::reader`
//! that returns exactly the visible sessions carrying the contributing rows):
//! - Scope: non-empty sessions (`is_empty = 0`); the time range applies to each message's own
//!   timestamp (`messages.ts_ms`), both bounds inclusive. A superseded session's own billed rows
//!   still count here, and drill-down maps them to its visible continuation successor.
//! - A "request" is a row matching [`billed_tokens_sql`]: a billed assistant message carrying
//!   tokens (main conversation + Subagents; cross-session duplicates count once on the billed
//!   session; unpriced models still count as requests).
//! - `hourly`: emitted only when the request specifies both bounds, `from_ms <= to_ms` (compared
//!   in unix milliseconds, not local wall clock), and `to_ms - from_ms <= 8 days`. Buckets span
//!   every local hour from `floor_hour(from)` through `floor_hour(to)`, skipping DST-gap hours
//!   and merging repeated DST-fold hours, with empty buckets included.
//!
//! Days and hours are local wall-clock time via `chrono::Local` (DST-correct, honours `TZ`).

use std::collections::{BTreeMap, HashSet};

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use rusqlite::{Connection, params_from_iter};

use crate::error::CoreResult;
use crate::index::schema::MSG_ASSISTANT;
use crate::model::{DailyModelTokens, HourUsage, Stats, StatsOverview, StatsRequest, TimeRange};

/// Label of messages without a model (the `Drill::Model` drill uses the same label).
pub const UNKNOWN: &str = "unknown";

/// Maximum inclusive range span (`to_ms - from_ms`) for which `hourly` buckets are returned.
const MAX_HOURLY_SPAN_MS: f64 = (8 * 24 * 3_600_000) as f64;

/// Local wall-clock time of a unix-ms timestamp.
pub(crate) fn local_time(ts_ms: i64) -> Option<NaiveDateTime> {
    Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .map(|d| d.naive_local())
}

/// Inclusive `[from, to]` bounds of an optional time range (unbounded sides are `i64` extremes).
pub(crate) fn range_bounds(r: Option<&TimeRange>) -> (i64, i64) {
    let from = r.and_then(|r| r.from_ms).map_or(i64::MIN, |v| v as i64);
    let to = r.and_then(|r| r.to_ms).map_or(i64::MAX, |v| v as i64);
    (from, to)
}

/// SQL predicate on a `messages` row: an assistant message that carries tokens and is billed
/// (the definition of a request for stats and drill-down). Columns are unqualified, so inside a
/// subquery over `messages` they resolve to its row.
pub(crate) fn billed_tokens_sql() -> String {
    format!(
        "role = {MSG_ASSISTANT} AND billed = 1
            AND (in_tok > 0 OR out_tok > 0 OR cr_tok > 0 OR cc_tok > 0)"
    )
}

#[derive(Default)]
struct DailyAgg {
    tokens: [f64; 4],
    cost: f64,
    requests: u32,
}

pub fn compute(conn: &Connection, r: &StatsRequest) -> CoreResult<Stats> {
    let (from, to) = range_bounds(r.time_range.as_ref());
    let ranged = r.time_range.is_some();

    let non_empty: HashSet<String> = {
        let mut st = conn.prepare("SELECT id FROM sessions WHERE is_empty = 0")?;
        let rows = st.query_map([], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };

    let mut output = 0f64;
    let mut cost = 0f64;
    let mut unpriced = 0u32;
    let mut requests = 0u32;
    let mut daily: BTreeMap<(NaiveDate, String), DailyAgg> = BTreeMap::new();
    let mut hourly_counts: BTreeMap<NaiveDateTime, (u32, f64)> = BTreeMap::new();
    let want_hourly = hourly_bounds(r.time_range.as_ref()).is_some();

    {
        let mut sql = format!(
            "SELECT session_id, ts_ms, model, in_tok, out_tok, cr_tok, cc_tok, cost
             FROM messages WHERE {billed}",
            billed = billed_tokens_sql()
        );
        // Unranged: a plain table scan beats walking `messages_ts` back into the table.
        let bounds = if ranged { vec![from, to] } else { vec![] };
        if ranged {
            sql.push_str(" AND ts_ms >= ?1 AND ts_ms <= ?2");
        }
        let mut st = conn.prepare(&sql)?;
        let mut rows = st.query(params_from_iter(bounds))?;
        while let Some(row) = rows.next()? {
            if !non_empty.contains(row.get_ref(0)?.as_str().unwrap_or_default()) {
                continue;
            }
            let Some(t) = local_time(row.get(1)?) else {
                continue;
            };
            let tok: [f64; 4] = [
                row.get::<_, i64>(3)? as f64,
                row.get::<_, i64>(4)? as f64,
                row.get::<_, i64>(5)? as f64,
                row.get::<_, i64>(6)? as f64,
            ];
            let msg_cost = row.get::<_, Option<f64>>(7)?;
            requests += 1;
            output += tok[1];
            match msg_cost {
                Some(c) => cost += c,
                None => unpriced += 1,
            }
            let added_cost = msg_cost.unwrap_or(0.0);
            let day = t.date();
            let model = row.get::<_, Option<String>>(2)?;
            let e = daily
                .entry((day, model.unwrap_or_else(|| UNKNOWN.to_owned())))
                .or_default();
            for (a, b) in e.tokens.iter_mut().zip(tok) {
                *a += b;
            }
            e.cost += added_cost;
            e.requests += 1;

            if want_hourly {
                let h = hourly_counts.entry(floor_hour(t)).or_default();
                h.0 += 1;
                h.1 += added_cost;
            }
        }
    }

    Ok(Stats {
        overview: StatsOverview {
            output_tokens: output,
            cost,
            unpriced_messages: unpriced,
            requests,
        },
        daily: daily
            .into_iter()
            .map(|((day, model), a)| DailyModelTokens {
                day: day.to_string(),
                model,
                input: a.tokens[0],
                output: a.tokens[1],
                cache_read: a.tokens[2],
                cache_creation: a.tokens[3],
                cost: a.cost,
                requests: a.requests,
            })
            .collect(),
        hourly: build_hourly(&hourly_counts, r.time_range.as_ref()),
    })
}

/// Start of the local hour holding `t`.
fn floor_hour(t: NaiveDateTime) -> NaiveDateTime {
    t.date().and_hms_opt(t.hour(), 0, 0).unwrap_or(t)
}

/// Unix ms of a local time: the first of the two instants of a time repeated when clocks go back
/// (so a bucket holds both readings of that hour, as `hourly_counts` does); a time skipped when
/// clocks go forward maps to the instant after the gap. Candidates are checked by converting back,
/// since chrono also offers the old offset right at the end of a repeated hour.
fn local_ms(t: NaiveDateTime) -> i64 {
    (0..3)
        .find_map(|h| {
            let wall = t + Duration::hours(h);
            let r = Local.from_local_datetime(&wall);
            [r.earliest(), r.latest()]
                .into_iter()
                .flatten()
                .filter(|d| d.with_timezone(&Local).naive_local() == wall)
                .map(|d| d.timestamp_millis())
                .min()
        })
        .unwrap_or(0)
}

/// Inclusive `(from_hour, to_hour)` when the request qualifies for hourly bucketing: both bounds
/// set, `from_ms <= to_ms` in raw `f64` unix milliseconds, and `to_ms - from_ms <= 8 days`.
fn hourly_bounds(range: Option<&TimeRange>) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let r = range?;
    let (from_f, to_f) = (r.from_ms?, r.to_ms?);
    if !from_f.is_finite() || !to_f.is_finite() {
        return None;
    }
    if from_f > to_f || (to_f - from_f) > MAX_HOURLY_SPAN_MS {
        return None;
    }
    let from_hour = floor_hour(local_time(from_f as i64)?);
    let to_hour = floor_hour(local_time(to_f as i64)?);
    Some((from_hour, to_hour))
}

fn build_hourly(
    hourly_counts: &BTreeMap<NaiveDateTime, (u32, f64)>,
    range: Option<&TimeRange>,
) -> Vec<HourUsage> {
    let Some((mut t, last)) = hourly_bounds(range) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while t <= last {
        let end = t + Duration::hours(1);
        let (start_ms, end_ms) = (local_ms(t), local_ms(end));
        // An hour skipped by a DST change is no time at all: no bucket (buckets stay contiguous).
        if end_ms > start_ms {
            let (requests, cost) = hourly_counts.get(&t).copied().unwrap_or((0, 0.0));
            out.push(HourUsage {
                start_ms: start_ms as f64,
                end_ms: end_ms as f64,
                requests,
                cost,
            });
        }
        t = end;
    }
    out
}
