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
//! - `hourly_models` and `sessions`: emitted only for single-day ranges (both bounds set,
//!   `from_ms <= to_ms` in raw unix milliseconds, and `to_ms - from_ms <= 26 hours`).
//!   `hourly_models` uses the same local-hour bucketing as `hourly` and includes only non-empty
//!   `(hour, model)` entries; `sessions` attributes superseded sessions' leftover rows to their
//!   visible continuation successor and merges consecutive request timestamps <= 10 minutes apart.
//!
//! Days and hours are local wall-clock time via `chrono::Local` (DST-correct, honours `TZ`).

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use rusqlite::{Connection, params_from_iter};

use crate::error::CoreResult;
use crate::index::schema::{MSG_ASSISTANT, SOURCE_CLAUDE};
use crate::model::{
    DailyModelTokens, HourModelUsage, HourUsage, SessionUsage, Stats, StatsOverview, StatsRequest,
    TimeRange, TimeSpan,
};

/// Label of messages without a model (the `Drill::Model` drill uses the same label).
pub const UNKNOWN: &str = "unknown";

/// Maximum inclusive range span (`to_ms - from_ms`) for which `hourly` buckets are returned.
const MAX_HOURLY_SPAN_MS: f64 = (8 * 24 * 3_600_000) as f64;

/// Maximum inclusive range span (`to_ms - from_ms`) for single-day breakdowns (`hourly_models` and
/// `sessions`).
const MAX_SINGLE_DAY_SPAN_MS: f64 = (26 * 3_600_000) as f64;

/// Maximum gap between two consecutive requests of a session to merge into one `TimeSpan`.
const MAX_SESSION_SPAN_GAP_MS: i64 = 10 * 60 * 1_000;

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
struct TokenAgg {
    tokens: [f64; 4],
    cost: f64,
    requests: u32,
}

impl TokenAgg {
    fn add(&mut self, tok: [f64; 4], cost: f64) {
        for (a, b) in self.tokens.iter_mut().zip(tok) {
            *a += b;
        }
        self.cost += cost;
        self.requests += 1;
    }
}

struct SessionRow {
    title: String,
    is_empty: bool,
    superseded: bool,
    continued_in_id: Option<String>,
}

#[derive(Default)]
struct SessionAgg {
    cost: f64,
    requests: u32,
    timestamps: Vec<i64>,
}

pub fn compute(conn: &Connection, r: &StatsRequest) -> CoreResult<Stats> {
    let (from, to) = range_bounds(r.time_range.as_ref());
    let ranged = r.time_range.is_some();
    let hourly_span = hourly_bounds(r.time_range.as_ref());
    let single_day_span = hourly_bounds_within(r.time_range.as_ref(), MAX_SINGLE_DAY_SPAN_MS);
    let want_hourly = hourly_span.is_some();
    let want_single_day = single_day_span.is_some();

    let (non_empty, sessions_meta): (HashSet<String>, HashMap<String, SessionRow>) =
        if want_single_day {
            let mut st = conn.prepare(&format!(
                "SELECT id, title, is_empty, superseded, continued_in_id FROM sessions
                 WHERE agent = '{SOURCE_CLAUDE}'"
            ))?;
            let rows = st.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    SessionRow {
                        title: row.get(1)?,
                        is_empty: row.get(2)?,
                        superseded: row.get(3)?,
                        continued_in_id: row.get(4)?,
                    },
                ))
            })?;
            let meta: HashMap<String, SessionRow> = rows.collect::<Result<_, _>>()?;
            (HashSet::new(), meta)
        } else {
            let mut st = conn.prepare(&format!(
                "SELECT id FROM sessions WHERE is_empty = 0 AND agent = '{SOURCE_CLAUDE}'"
            ))?;
            let rows = st.query_map([], |row| row.get(0))?;
            (rows.collect::<Result<_, _>>()?, HashMap::new())
        };

    let mut output = 0f64;
    let mut cost = 0f64;
    let mut unpriced = 0u32;
    let mut requests = 0u32;
    let mut daily: BTreeMap<(NaiveDate, String), TokenAgg> = BTreeMap::new();
    let mut hourly_counts: BTreeMap<NaiveDateTime, (u32, f64)> = BTreeMap::new();
    let mut hourly_models_map: BTreeMap<NaiveDateTime, BTreeMap<String, TokenAgg>> =
        BTreeMap::new();
    let mut session_aggs: HashMap<String, SessionAgg> = HashMap::new();

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
            let sid = row.get_ref(0)?.as_str().unwrap_or_default();
            let is_non_empty = if want_single_day {
                sessions_meta.get(sid).is_some_and(|s| !s.is_empty)
            } else {
                non_empty.contains(sid)
            };
            if !is_non_empty {
                continue;
            }
            let ts_ms: i64 = row.get(1)?;
            let Some(t) = local_time(ts_ms) else {
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
            let model = row
                .get::<_, Option<String>>(2)?
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| UNKNOWN.to_owned());

            if want_hourly {
                let hour = floor_hour(t);
                let h = hourly_counts.entry(hour).or_default();
                h.0 += 1;
                h.1 += added_cost;

                if want_single_day {
                    hourly_models_map
                        .entry(hour)
                        .or_default()
                        .entry(model.clone())
                        .or_default()
                        .add(tok, added_cost);
                    if let Some(owner) = visible_owner(&sessions_meta, sid) {
                        let s = session_aggs.entry(owner.to_owned()).or_default();
                        s.cost += added_cost;
                        s.requests += 1;
                        s.timestamps.push(ts_ms);
                    }
                }
            }

            daily.entry((day, model)).or_default().add(tok, added_cost);
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
        hourly: build_hourly(&hourly_counts, hourly_span),
        hourly_models: build_hourly_models(&hourly_models_map, single_day_span),
        sessions: if want_single_day {
            build_sessions(session_aggs, &sessions_meta)
        } else {
            Vec::new()
        },
    })
}

/// Resolves a non-empty session id to the visible (`is_empty = 0`, `superseded = 0`) session that
/// owns it by following `continued_in_id` links through superseded sessions. Returns `None` if the
/// chain breaks, cycles, or ends on a hidden/missing session.
fn visible_owner<'a>(sessions: &'a HashMap<String, SessionRow>, start: &'a str) -> Option<&'a str> {
    let mut cur = start;
    let mut seen: Vec<&str> = Vec::new();
    loop {
        let info = sessions.get(cur)?;
        if !info.superseded {
            return (!info.is_empty).then_some(cur);
        }
        let next = info.continued_in_id.as_deref()?;
        if next == start || seen.contains(&next) {
            return None;
        }
        seen.push(cur);
        cur = next;
    }
}

fn build_spans(mut timestamps: Vec<i64>) -> Vec<TimeSpan> {
    timestamps.sort_unstable();
    let mut spans: Vec<(i64, i64)> = Vec::new();
    for ts in timestamps {
        match spans.last_mut() {
            Some((_, end)) if ts.saturating_sub(*end) <= MAX_SESSION_SPAN_GAP_MS => {
                *end = ts;
            }
            _ => spans.push((ts, ts)),
        }
    }
    spans
        .into_iter()
        .map(|(start, end)| TimeSpan {
            start_ms: start as f64,
            end_ms: end as f64,
        })
        .collect()
}

fn build_sessions(
    aggs: HashMap<String, SessionAgg>,
    meta: &HashMap<String, SessionRow>,
) -> Vec<SessionUsage> {
    let mut out: Vec<SessionUsage> = aggs
        .into_iter()
        .map(|(session_id, agg)| {
            let title = meta
                .get(&session_id)
                .map(|m| m.title.clone())
                .unwrap_or_default();
            SessionUsage {
                session_id,
                title,
                cost: agg.cost,
                requests: agg.requests,
                spans: build_spans(agg.timestamps),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.cost
            .total_cmp(&a.cost)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    out
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

/// Inclusive `(from_hour, to_hour)` when the request specifies both bounds, `from_ms <= to_ms` in
/// raw `f64` unix milliseconds, and `to_ms - from_ms <= max_span_ms`.
fn hourly_bounds_within(
    range: Option<&TimeRange>,
    max_span_ms: f64,
) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let r = range?;
    let (from_f, to_f) = (r.from_ms?, r.to_ms?);
    if !from_f.is_finite() || !to_f.is_finite() {
        return None;
    }
    if from_f > to_f || (to_f - from_f) > max_span_ms {
        return None;
    }
    let from_hour = floor_hour(local_time(from_f as i64)?);
    let to_hour = floor_hour(local_time(to_f as i64)?);
    Some((from_hour, to_hour))
}

/// Inclusive `(from_hour, to_hour)` when the request qualifies for hourly bucketing (<= 8 days).
fn hourly_bounds(range: Option<&TimeRange>) -> Option<(NaiveDateTime, NaiveDateTime)> {
    hourly_bounds_within(range, MAX_HOURLY_SPAN_MS)
}

fn build_hourly(
    hourly_counts: &BTreeMap<NaiveDateTime, (u32, f64)>,
    bounds: Option<(NaiveDateTime, NaiveDateTime)>,
) -> Vec<HourUsage> {
    let Some((mut t, last)) = bounds else {
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

fn build_hourly_models(
    hourly_models: &BTreeMap<NaiveDateTime, BTreeMap<String, TokenAgg>>,
    bounds: Option<(NaiveDateTime, NaiveDateTime)>,
) -> Vec<HourModelUsage> {
    let Some((mut t, last)) = bounds else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while t <= last {
        let end = t + Duration::hours(1);
        let (start_ms, end_ms) = (local_ms(t), local_ms(end));
        if end_ms > start_ms
            && let Some(models) = hourly_models.get(&t)
        {
            for (model, a) in models {
                out.push(HourModelUsage {
                    start_ms: start_ms as f64,
                    model: model.clone(),
                    input: a.tokens[0],
                    output: a.tokens[1],
                    cache_read: a.tokens[2],
                    cache_creation: a.tokens[3],
                    cost: a.cost,
                    requests: a.requests,
                });
            }
        }
        t = end;
    }
    out
}
