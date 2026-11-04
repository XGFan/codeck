//! Stats panel aggregation (local-time buckets: days and activity bars; A13).
//!
//! Semantics (each chart dimension has a [`Drill`](crate::model::Drill) in `index::reader` that
//! returns exactly the contributing sessions):
//! - Scope: non-empty sessions; the time range applies to each message's own timestamp
//!   (`messages.ts_ms`), both bounds inclusive.
//! - `overview.messages`, `heat_daily`, `overview.sessions`,
//!   `overview.active_days` and the ranked sessions' `messages`: main-file messages only
//!   (human/command prompts + assistant messages by `message.id`, as `message_count`).
//! - Tokens (`overview.output_tokens`, `daily`, session `output_tokens`) and cost
//!   (`overview.cost`, session `cost`, see `pricing`): main + subagent messages, each billed
//!   once (`messages.billed`, see `writer::rebill`); `daily` skips messages without tokens
//!   (prompts, `<synthetic>` errors).
//! - `activity`: main-file messages per bar over the range (from the earliest message when it has no
//!   lower bound, to now when it has no upper bound); see [`choose_bucket`].
//!
//! Days and hours are local wall-clock time via `chrono::Local` (DST-correct, honours `TZ`).

use std::collections::{BTreeMap, HashMap};

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use rusqlite::{Connection, params_from_iter};

use crate::error::CoreResult;
use crate::index::schema::MSG_ASSISTANT;
use crate::model::{
    Activity, ActivityBar, ActivityBucket, DailyModelTokens, DayCount, SessionStat, Stats,
    StatsOverview, StatsRequest, TimeRange,
};

/// Label of messages without a model and Subagent Runs without an `agentType`
/// (the matching drills use the same label).
pub const UNKNOWN: &str = "unknown";

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

/// Length of the session ranking.
pub const TOP_SESSIONS: usize = 10;

pub fn compute(conn: &Connection, r: &StatsRequest) -> CoreResult<Stats> {
    let (from, to) = range_bounds(r.time_range.as_ref());
    let ranged = r.time_range.is_some();

    // Sessions in scope → index, with their titles.
    let mut sessions: HashMap<String, usize> = HashMap::new();
    let mut titles: Vec<(String, String)> = Vec::new();
    {
        let mut st = conn.prepare("SELECT id, title FROM sessions WHERE is_empty = 0")?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            sessions.insert(id.clone(), titles.len());
            titles.push((id, row.get(1)?));
        }
    }

    // One pass over the messages in range.
    let mut session_active = vec![false; sessions.len()];
    let mut sess_messages = vec![0u32; sessions.len()];
    let mut sess_output = vec![0f64; sessions.len()];
    let mut sess_cost = vec![0f64; sessions.len()];
    let mut messages = 0u32;
    let mut output = 0f64;
    let mut cost = 0f64;
    let mut unpriced = 0u32;
    let mut heat_daily: BTreeMap<NaiveDate, u32> = BTreeMap::new();
    // Main-file messages per local hour, the unit the activity bars are summed from.
    let mut hourly: BTreeMap<NaiveDateTime, u32> = BTreeMap::new();
    let mut daily: BTreeMap<(NaiveDate, String), [f64; 4]> = BTreeMap::new();
    {
        let mut sql = "SELECT session_id, agent_id = '', ts_ms, role, model, in_tok, out_tok,
            cr_tok, cc_tok, cost, billed FROM messages"
            .to_owned();
        // Unranged: a plain table scan beats walking `messages_ts` back into the table.
        let bounds = if ranged { vec![from, to] } else { vec![] };
        if ranged {
            sql.push_str(" WHERE ts_ms >= ?1 AND ts_ms <= ?2");
        }
        let mut st = conn.prepare(&sql)?;
        let mut rows = st.query(params_from_iter(bounds))?;
        while let Some(row) = rows.next()? {
            let Some(&si) = sessions.get(row.get_ref(0)?.as_str().unwrap_or_default()) else {
                continue;
            };
            let Some(t) = local_time(row.get(2)?) else {
                continue;
            };
            let day = t.date();
            if row.get::<_, bool>(1)? {
                session_active[si] = true;
                sess_messages[si] += 1;
                messages += 1;
                *heat_daily.entry(day).or_default() += 1;
                *hourly.entry(floor(ActivityBucket::Hour1, t)).or_default() += 1;
            }
            let tok: [f64; 4] = [
                row.get::<_, i64>(5)? as f64,
                row.get::<_, i64>(6)? as f64,
                row.get::<_, i64>(7)? as f64,
                row.get::<_, i64>(8)? as f64,
            ];
            if row.get::<_, i64>(3)? != MSG_ASSISTANT
                || tok.iter().all(|&v| v == 0.0)
                || !row.get::<_, bool>(10)?
            {
                continue;
            }
            output += tok[1];
            sess_output[si] += tok[1];
            match row.get::<_, Option<f64>>(9)? {
                Some(c) => {
                    cost += c;
                    sess_cost[si] += c;
                }
                None => unpriced += 1,
            }
            let model = row.get::<_, Option<String>>(4)?;
            let e = daily
                .entry((day, model.unwrap_or_else(|| UNKNOWN.to_owned())))
                .or_default();
            for (a, b) in e.iter_mut().zip(tok) {
                *a += b;
            }
        }
    }

    let mut top: Vec<SessionStat> = titles
        .into_iter()
        .enumerate()
        .filter(|&(i, _)| sess_messages[i] > 0 || sess_output[i] > 0.0)
        .map(|(i, (session_id, title))| SessionStat {
            session_id,
            title,
            messages: sess_messages[i],
            output_tokens: sess_output[i],
            cost: sess_cost[i],
        })
        .collect();
    top.sort_by(|a, b| {
        b.cost
            .total_cmp(&a.cost)
            .then(b.output_tokens.total_cmp(&a.output_tokens))
            .then(a.session_id.cmp(&b.session_id))
    });
    top.truncate(TOP_SESSIONS);

    Ok(Stats {
        overview: StatsOverview {
            sessions: session_active.iter().filter(|&&a| a).count() as u32,
            messages,
            output_tokens: output,
            active_days: heat_daily.len() as u32,
            cost,
            unpriced_messages: unpriced,
        },
        daily: daily
            .into_iter()
            .map(|((day, model), t)| DailyModelTokens {
                day: day.to_string(),
                model,
                input: t[0],
                output: t[1],
                cache_read: t[2],
                cache_creation: t[3],
            })
            .collect(),
        heat_daily: heat_daily
            .into_iter()
            .map(|(day, messages)| DayCount {
                day: day.to_string(),
                messages,
            })
            .collect(),
        activity: activity(&hourly, r.time_range.as_ref()),
        sessions: top,
    })
}

/// Most bars in the activity histogram.
pub const MAX_ACTIVITY_BARS: usize = 170;

const BUCKETS: [ActivityBucket; 7] = [
    ActivityBucket::Hour1,
    ActivityBucket::Hour3,
    ActivityBucket::Hour6,
    ActivityBucket::Hour12,
    ActivityBucket::Day,
    ActivityBucket::Week,
    ActivityBucket::Month,
];

/// Start of the bucket holding the local time `t`.
fn floor(b: ActivityBucket, t: NaiveDateTime) -> NaiveDateTime {
    let midnight = t.date().and_hms_opt(0, 0, 0).unwrap_or(t);
    let hours = |n: u32| midnight + Duration::hours(i64::from(t.hour() - t.hour() % n));
    match b {
        ActivityBucket::Hour1 => hours(1),
        ActivityBucket::Hour3 => hours(3),
        ActivityBucket::Hour6 => hours(6),
        ActivityBucket::Hour12 => hours(12),
        ActivityBucket::Day => midnight,
        ActivityBucket::Week => {
            midnight - Duration::days(i64::from(t.weekday().num_days_from_monday()))
        }
        ActivityBucket::Month => midnight.with_day(1).unwrap_or(midnight),
    }
}

/// Start of the bucket after the one starting at `t`.
fn next(b: ActivityBucket, t: NaiveDateTime) -> NaiveDateTime {
    match b {
        ActivityBucket::Hour1 => t + Duration::hours(1),
        ActivityBucket::Hour3 => t + Duration::hours(3),
        ActivityBucket::Hour6 => t + Duration::hours(6),
        ActivityBucket::Hour12 => t + Duration::hours(12),
        ActivityBucket::Day => t + Duration::days(1),
        ActivityBucket::Week => t + Duration::days(7),
        ActivityBucket::Month => t
            .checked_add_months(chrono::Months::new(1))
            .unwrap_or(t + Duration::days(31)),
    }
}

/// Unix ms of a local time: the first of the two instants of a time repeated when clocks go back
/// (so a bar holds both readings of that hour, as the per-hour counts do); a time skipped when
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

/// The finest bucket that covers `[from, to]` (local times) in at most [`MAX_ACTIVITY_BARS`] bars.
pub fn choose_bucket(from: NaiveDateTime, to: NaiveDateTime) -> ActivityBucket {
    BUCKETS
        .into_iter()
        .find(|&b| {
            let mut t = floor(b, from);
            let mut n = 0;
            while t <= to && n <= MAX_ACTIVITY_BARS {
                t = next(b, t);
                n += 1;
            }
            n <= MAX_ACTIVITY_BARS
        })
        .unwrap_or(ActivityBucket::Month)
}

/// The histogram over the request's range from per-hour counts; empty when unbounded and there is
/// no message.
fn activity(hourly: &BTreeMap<NaiveDateTime, u32>, range: Option<&TimeRange>) -> Activity {
    let from = range
        .and_then(|r| r.from_ms)
        .and_then(|ms| local_time(ms as i64))
        .or_else(|| hourly.keys().next().copied());
    // Unbounded: up to now, or to the latest message if a clock ahead of ours dated it later.
    let to = range
        .and_then(|r| r.to_ms)
        .and_then(|ms| local_time(ms as i64))
        .unwrap_or_else(|| {
            let now = Local::now().naive_local();
            hourly.keys().next_back().map_or(now, |&last| last.max(now))
        });
    let Some(from) = from.filter(|f| *f <= to) else {
        return Activity::default();
    };
    let bucket = choose_bucket(from, to);
    // Month bars are the coarsest: past that, keep the latest MAX_ACTIVITY_BARS months (an absurd
    // timestamp such as 1970 must not stretch the axis over decades).
    let from = if bucket == ActivityBucket::Month {
        to.checked_sub_months(chrono::Months::new(MAX_ACTIVITY_BARS as u32 - 1))
            .map_or(from, |cap| from.max(cap))
    } else {
        from
    };
    let mut bars = Vec::new();
    let mut t = floor(bucket, from);
    while t <= to {
        let end = next(bucket, t);
        let (start_ms, end_ms) = (local_ms(t), local_ms(end));
        // An hour skipped by a DST change is no time at all: no bar (bars stay contiguous).
        if end_ms > start_ms {
            bars.push(ActivityBar {
                start_ms: start_ms as f64,
                end_ms: end_ms as f64,
                messages: hourly.range(t..end).map(|(_, n)| n).sum(),
            });
        }
        t = end;
    }
    Activity { bucket, bars }
}
