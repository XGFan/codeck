//! `list_projects`, `list_sessions` (sorts, drill filters) and session detail rows.

use std::collections::{HashMap, HashSet};

use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use super::schema::MSG_ASSISTANT;
use super::writer::{role_from_db, title_source_from_db};
use crate::assemble::AgentStats;
use crate::error::CoreResult;
use crate::model::{
    Drill, FileRole, ForkChild, ForkOrigin, ModelCost, ProjectSummary, SessionQuery, SessionSort,
    SessionSummary, TokenTotals,
};
use crate::stats;

/// A `files` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    pub id: i64,
    pub path: String,
    pub dev: u64,
    pub ino: u64,
    pub role: FileRole,
    pub session_id: String,
    pub agent_id: Option<String>,
    pub size: u64,
    pub mtime_ns: i64,
    pub head: Vec<u8>,
    pub parsed_offset: u64,
    pub text_offset: u64,
    pub line_count: u64,
    pub failed_lines: u64,
    pub first_error: Option<String>,
}

const FILE_COLS: &str = "id, path, dev, ino, role, session_id, agent_id, size, mtime_ns, head, \
    parsed_offset, text_offset, line_count, failed_lines, first_error";

fn file_from_row(r: &Row) -> rusqlite::Result<FileRecord> {
    Ok(FileRecord {
        id: r.get(0)?,
        path: r.get(1)?,
        dev: r.get::<_, i64>(2)? as u64,
        ino: r.get::<_, i64>(3)? as u64,
        role: role_from_db(r.get(4)?),
        session_id: r.get(5)?,
        agent_id: r.get(6)?,
        size: r.get::<_, i64>(7)? as u64,
        mtime_ns: r.get(8)?,
        head: r.get(9)?,
        parsed_offset: r.get::<_, i64>(10)? as u64,
        text_offset: r.get::<_, i64>(11)? as u64,
        line_count: r.get::<_, i64>(12)? as u64,
        failed_lines: r.get::<_, i64>(13)? as u64,
        first_error: r.get(14)?,
    })
}

pub fn all_files(conn: &Connection) -> CoreResult<Vec<FileRecord>> {
    let mut st = conn.prepare(&format!("SELECT {FILE_COLS} FROM files"))?;
    let rows = st.query_map([], file_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Files of a session, largest main file first, then agent files by path.
pub fn session_files(conn: &Connection, session_id: &str) -> CoreResult<Vec<FileRecord>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {FILE_COLS} FROM files WHERE session_id=?1 ORDER BY role, size DESC, path"
    ))?;
    let rows = st.query_map([session_id], file_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// (session id, newest main-file mtime in ms) for the recent-write fallback.
pub fn main_mtimes(conn: &Connection, since_ms: f64) -> CoreResult<Vec<(String, f64)>> {
    let mut st = conn.prepare_cached(
        "SELECT session_id, max(mtime_ns) FROM files WHERE role=0 GROUP BY session_id
         HAVING max(mtime_ns) >= ?1",
    )?;
    let since_ns = (since_ms as i64).saturating_mul(1_000_000);
    let rows = st.query_map([since_ns], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as f64 / 1e6))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Projects with at least one non-empty session started in or editing them, most recently active
/// first.
pub fn list_projects(conn: &Connection) -> CoreResult<Vec<ProjectSummary>> {
    let mut st = conn.prepare(
        "WITH rel AS (SELECT id AS session_id, project_id FROM sessions
                UNION SELECT session_id, project_id FROM session_projects)
         SELECT p.id, p.path, p.display_name, p.missing, count(s.id), max(s.last_active_ms)
         FROM projects p JOIN rel ON rel.project_id = p.id
            JOIN sessions s ON s.id = rel.session_id AND s.is_empty = 0
         GROUP BY p.id ORDER BY max(s.last_active_ms) DESC, p.id",
    )?;
    let rows = st.query_map([], |r| {
        Ok(ProjectSummary {
            id: r.get(0)?,
            path: r.get(1)?,
            display_name: r.get(2)?,
            missing: r.get(3)?,
            session_count: r.get(4)?,
            last_active_ms: r.get::<_, i64>(5)? as f64,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Origin of a session: `forkedFrom`, else the latest earlier session sharing its `root_uuid`
/// (§3.4 step 8 fallback). `s` is the alias of the session row.
fn origin_expr(s: &str) -> String {
    format!(
        "coalesce({s}.fork_origin_id, (SELECT o2.id FROM sessions o2 WHERE {s}.root_uuid IS NOT NULL
            AND o2.root_uuid = {s}.root_uuid AND o2.id != {s}.id AND o2.created_ms < {s}.created_ms
            ORDER BY o2.created_ms DESC LIMIT 1))"
    )
}

fn summary_select(where_sql: &str) -> String {
    format!(
        "WITH b AS (SELECT s.*, {origin} AS origin FROM sessions s WHERE {where_sql})
         SELECT b.id, b.project_id, b.title, b.title_source, b.created_ms, b.last_active_ms,
            b.message_count, b.tool_call_count, b.subagent_count, b.in_tok, b.out_tok, b.cr_tok,
            b.cc_tok, b.git_branch, b.primary_model, b.origin, o.title,
            CASE WHEN b.origin = b.fork_origin_id THEN b.fork_point_uuid END, b.cost, b.unpriced,
            (SELECT json_group_array(sp.project_id) FROM session_projects sp WHERE sp.session_id = b.id)
         FROM b LEFT JOIN sessions o ON o.id = b.origin",
        origin = origin_expr("s")
    )
}

fn summary_from_row(r: &Row) -> rusqlite::Result<SessionSummary> {
    let last_active = r.get::<_, i64>(5)? as f64;
    let origin: Option<String> = r.get(15)?;
    Ok(SessionSummary {
        id: r.get(0)?,
        project_id: r.get(1)?,
        title: r.get(2)?,
        title_source: title_source_from_db(r.get(3)?),
        created_ms: r
            .get::<_, Option<i64>>(4)?
            .map_or(last_active, |v| v as f64),
        last_active_ms: last_active,
        message_count: r.get(6)?,
        tool_call_count: r.get(7)?,
        subagent_count: r.get(8)?,
        tokens: TokenTotals {
            input: r.get::<_, i64>(9)? as f64,
            output: r.get::<_, i64>(10)? as f64,
            cache_read: r.get::<_, i64>(11)? as f64,
            cache_creation: r.get::<_, i64>(12)? as f64,
        },
        cost: r.get(18)?,
        unpriced_messages: r.get(19)?,
        git_branch: r.get(13)?,
        live: None,
        fork_origin: match origin {
            Some(session_id) => Some(ForkOrigin {
                session_id,
                title: r.get(16)?,
                fork_point_id: r.get(17)?,
            }),
            None => None,
        },
        primary_model: r.get(14)?,
        edited_project_ids: serde_json::from_str(&r.get::<_, String>(20)?).unwrap_or_default(),
    })
}

/// `(s.project_id IN (…) OR <s edited one of them>)` for a session alias `s` (appends the
/// arguments): a session belongs to the Project it started in and to every Project it edited.
pub fn project_match(ids: &[String], args: &mut Vec<Value>) -> String {
    let marks = vec!["?"; ids.len()].join(",");
    for _ in 0..2 {
        args.extend(ids.iter().map(|p| Value::Text(p.clone())));
    }
    format!(
        "(s.project_id IN ({marks}) OR s.id IN
            (SELECT sp.session_id FROM session_projects sp WHERE sp.project_id IN ({marks})))"
    )
}

/// Non-empty sessions matching `q` (live filtering and `live` states are applied by the engine).
///
/// Without a drill, the time range keeps sessions whose span overlaps it. With a drill, the
/// session must have a contributing row inside the range, mirroring `stats::compute` exactly:
/// - `Day`: a main-file message on that local day;
/// - `Model`: a main or Subagent message of that model that carries tokens;
/// - `Span`: a main-file message in `[from_ms, to_ms)` (an activity bar).
pub fn list_sessions(conn: &Connection, q: &SessionQuery) -> CoreResult<Vec<SessionSummary>> {
    let mut conds = vec!["s.is_empty = 0".to_owned()];
    let mut args: Vec<Value> = Vec::new();
    if !q.project_ids.is_empty() {
        conds.push(project_match(&q.project_ids, &mut args));
    }
    let (from, to) = stats::range_bounds(q.time_range.as_ref());
    // `AND <col> BETWEEN from AND to` for drill rows (only when a range is set, so rows without
    // a timestamp still count otherwise).
    let ranged = q.time_range.is_some();
    let in_range = |col: &str, args: &mut Vec<Value>| {
        if !ranged {
            return String::new();
        }
        args.push(Value::Integer(from));
        args.push(Value::Integer(to));
        format!(" AND {col} >= ? AND {col} <= ?")
    };
    if q.drill.is_none()
        && let Some(tr) = &q.time_range
    {
        if let Some(from) = tr.from_ms {
            conds.push("s.last_active_ms >= ?".into());
            args.push(Value::Integer(from as i64));
        }
        if let Some(to) = tr.to_ms {
            conds.push("coalesce(s.created_ms, s.last_active_ms) <= ?".into());
            args.push(Value::Integer(to as i64));
        }
    }
    match &q.drill {
        Some(Drill::Day { day }) => match local_day_bounds_ms(day) {
            Some((start, end)) => {
                conds.push(
                    "s.id IN (SELECT m.session_id FROM messages m WHERE m.agent_id = ''
                        AND m.ts_ms >= ? AND m.ts_ms <= ?)"
                        .into(),
                );
                args.push(Value::Integer(start.max(from)));
                args.push(Value::Integer((end - 1).min(to)));
            }
            None => conds.push("0".into()),
        },
        Some(Drill::Span { from_ms, to_ms }) => {
            conds.push(
                "s.id IN (SELECT m.session_id FROM messages m WHERE m.agent_id = ''
                    AND m.ts_ms >= ? AND m.ts_ms <= ?)"
                    .into(),
            );
            args.push(Value::Integer((*from_ms as i64).max(from)));
            args.push(Value::Integer((*to_ms as i64 - 1).min(to)));
        }
        Some(Drill::Model { model }) => {
            args.push(Value::Text(stats::UNKNOWN.to_owned()));
            args.push(Value::Text(model.clone()));
            let range = in_range("m.ts_ms", &mut args);
            conds.push(format!(
                "EXISTS (SELECT 1 FROM messages m WHERE m.session_id = s.id
                    AND coalesce(m.model, ?) = ? AND m.role = {MSG_ASSISTANT} AND m.billed = 1
                    AND (m.in_tok > 0 OR m.out_tok > 0 OR m.cr_tok > 0 OR m.cc_tok > 0){range})"
            ));
        }
        None => {}
    }
    let col = match q.sort {
        SessionSort::LastActive => "b.last_active_ms",
        SessionSort::Created => "coalesce(b.created_ms, b.last_active_ms)",
        SessionSort::Messages => "b.message_count",
        SessionSort::Tokens => "b.out_tok",
        SessionSort::Cost => "b.cost",
    };
    let dir = if q.descending { "DESC" } else { "ASC" };
    let sql = format!(
        "{} ORDER BY {col} {dir}, b.id {dir}",
        summary_select(&conds.join(" AND "))
    );
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(args), summary_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `[start, end)` of a local calendar day (`YYYY-MM-DD`) in unix ms.
fn local_day_bounds_ms(day: &str) -> Option<(i64, i64)> {
    let d = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let start = Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    let next = d.succ_opt()?;
    let end = Local
        .from_local_datetime(&next.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    Some((start.timestamp_millis(), end.timestamp_millis()))
}

/// Extra columns of `get_session`.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionDetailRow {
    pub summary: SessionSummary,
    pub cwd: Option<String>,
    pub project_path: String,
    pub project_missing: bool,
    pub duration_ms: f64,
    pub models: Vec<String>,
    pub versions: Vec<String>,
    pub tokens_main: TokenTotals,
    pub root_uuid: Option<String>,
}

pub fn session_detail(conn: &Connection, id: &str) -> CoreResult<Option<SessionDetailRow>> {
    let sql = summary_select("s.id = ?1");
    let Some(summary) = conn.query_row(&sql, [id], summary_from_row).optional()? else {
        return Ok(None);
    };
    let row = conn.query_row(
        "SELECT s.cwd, coalesce(p.path, s.project_id), coalesce(p.missing, 1), s.duration_ms,
            s.models_json, s.versions_json, s.main_in_tok, s.main_out_tok, s.main_cr_tok,
            s.main_cc_tok, s.root_uuid
         FROM sessions s LEFT JOIN projects p ON p.id = s.project_id WHERE s.id = ?1",
        [id],
        |r| {
            Ok(SessionDetailRow {
                summary: summary.clone(),
                cwd: r.get(0)?,
                project_path: r.get(1)?,
                project_missing: r.get(2)?,
                duration_ms: r.get::<_, i64>(3)? as f64,
                models: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                versions: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
                tokens_main: TokenTotals {
                    input: r.get::<_, i64>(6)? as f64,
                    output: r.get::<_, i64>(7)? as f64,
                    cache_read: r.get::<_, i64>(8)? as f64,
                    cache_creation: r.get::<_, i64>(9)? as f64,
                },
                root_uuid: r.get(10)?,
            })
        },
    )?;
    Ok(Some(row))
}

/// Cost per model of a session (main + subagents), most expensive first.
pub fn model_costs(conn: &Connection, session_id: &str) -> CoreResult<Vec<ModelCost>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT coalesce(model, ?2), coalesce(sum(cost), 0), count(*) FILTER (WHERE cost IS NULL)
         FROM messages WHERE session_id = ?1 AND role = {MSG_ASSISTANT} AND billed = 1
            AND (in_tok > 0 OR out_tok > 0 OR cr_tok > 0 OR cc_tok > 0)
         GROUP BY 1 ORDER BY 2 DESC, 1"
    ))?;
    let rows = st.query_map(params![session_id, stats::UNKNOWN], |r| {
        Ok(ModelCost {
            model: r.get(0)?,
            cost: r.get(1)?,
            unpriced_messages: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Sessions whose Origin Session is `id` (via `forkedFrom` or the shared-root fallback).
pub fn fork_children(conn: &Connection, id: &str) -> CoreResult<Vec<ForkChild>> {
    let sql = format!(
        "SELECT c.id, c.title, coalesce(c.created_ms, c.last_active_ms) FROM sessions c
         WHERE c.is_empty = 0 AND c.id != ?1
            AND (c.fork_origin_id = ?1 OR (c.fork_origin_id IS NULL AND {origin} = ?1))
         ORDER BY coalesce(c.created_ms, c.last_active_ms)",
        origin = origin_expr("c")
    );
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map([id], |r| {
        Ok(ForkChild {
            session_id: r.get(0)?,
            title: r.get(1)?,
            created_ms: r.get::<_, i64>(2)? as f64,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Fork origin of a session (same rule as the summaries).
pub fn fork_origin(conn: &Connection, id: &str) -> CoreResult<Option<ForkOrigin>> {
    let sql = summary_select("s.id = ?1");
    Ok(conn
        .query_row(&sql, [id], summary_from_row)
        .optional()?
        .and_then(|s| s.fork_origin))
}

/// A8: per-agent figures from `messages` / `tool_calls`.
pub fn agent_stats(conn: &Connection, session_id: &str) -> CoreResult<HashMap<String, AgentStats>> {
    let mut out: HashMap<String, AgentStats> = HashMap::new();
    let mut st = conn.prepare_cached(
        "SELECT agent_id, count(*), sum(in_tok), sum(out_tok), sum(cr_tok), sum(cc_tok), min(ts_ms), max(ts_ms)
         FROM messages WHERE session_id=?1 AND agent_id != '' GROUP BY agent_id",
    )?;
    let mut rows = st.query([session_id])?;
    while let Some(r) = rows.next()? {
        out.insert(
            r.get(0)?,
            AgentStats {
                message_count: r.get(1)?,
                tool_call_count: 0,
                tokens: TokenTotals {
                    input: r.get::<_, i64>(2)? as f64,
                    output: r.get::<_, i64>(3)? as f64,
                    cache_read: r.get::<_, i64>(4)? as f64,
                    cache_creation: r.get::<_, i64>(5)? as f64,
                },
                started_ms: r.get::<_, Option<i64>>(6)?.map(|v| v as f64),
                ended_ms: r.get::<_, Option<i64>>(7)?.map(|v| v as f64),
                model: None,
            },
        );
    }
    let mut st = conn.prepare_cached(
        "SELECT agent_id, model, count(*) AS c FROM messages
         WHERE session_id=?1 AND agent_id != '' AND model IS NOT NULL
         GROUP BY agent_id, model ORDER BY c DESC",
    )?;
    let mut rows = st.query([session_id])?;
    while let Some(r) = rows.next()? {
        let agent: String = r.get(0)?;
        let e = out.entry(agent).or_default();
        if e.model.is_none() {
            e.model = r.get(1)?;
        }
    }
    let mut st = conn.prepare_cached(
        "SELECT agent_id, count(*) FROM tool_calls WHERE session_id=?1 AND agent_id != ''
         GROUP BY agent_id",
    )?;
    let mut rows = st.query([session_id])?;
    while let Some(r) = rows.next()? {
        let agent: String = r.get(0)?;
        out.entry(agent).or_default().tool_call_count = r.get(1)?;
    }
    Ok(out)
}

/// Number of non-empty sessions.
pub fn session_count(conn: &Connection) -> CoreResult<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM sessions WHERE is_empty = 0",
        [],
        |r| r.get(0),
    )?)
}

/// Phase 2 is done: no file has text left to index.
pub fn text_ready(conn: &Connection) -> CoreResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM files WHERE text_offset < parsed_offset LIMIT 1",
            params![],
            |_| Ok(()),
        )
        .optional()?
        .is_none())
}

/// Sessions that have `msg_text` rows.
pub fn sessions_with_text(conn: &Connection) -> CoreResult<HashSet<String>> {
    let mut st = conn.prepare("SELECT DISTINCT session_id FROM msg_text")?;
    let rows = st.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}
