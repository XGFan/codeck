//! `list_projects`, `list_sessions` (sorts, drill filters) and session detail rows.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use super::schema::{MSG_PROMPT, SOURCE_AGY, SOURCE_CLAUDE};
use super::writer::{
    agent_from_db, role_from_db, source_from_db, source_to_db, title_source_from_db,
};
use crate::error::CoreResult;
use crate::model::{
    Drill, FileRole, ForkChild, ForkOrigin, ModelCost, ProjectSummary, SessionQuery, SessionSort,
    SessionSummary, TokenTotals,
};
use crate::scan::FileSource;
use crate::stats::{self, billed_tokens_sql};
use transcript::assemble::AgentStats;

/// A `files` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    pub id: i64,
    pub path: String,
    pub dev: u64,
    pub ino: u64,
    pub source: FileSource,
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

const FILE_COLS: &str = "id, path, dev, ino, source, role, session_id, agent_id, size, mtime_ns, \
    head, parsed_offset, text_offset, line_count, failed_lines, first_error";

fn file_from_row(r: &Row) -> rusqlite::Result<FileRecord> {
    let source: String = r.get(4)?;
    Ok(FileRecord {
        id: r.get(0)?,
        path: r.get(1)?,
        dev: r.get::<_, i64>(2)? as u64,
        ino: r.get::<_, i64>(3)? as u64,
        source: source_from_db(&source),
        role: role_from_db(r.get(5)?),
        session_id: r.get(6)?,
        agent_id: r.get(7)?,
        size: r.get::<_, i64>(8)? as u64,
        mtime_ns: r.get(9)?,
        head: r.get(10)?,
        parsed_offset: r.get::<_, i64>(11)? as u64,
        text_offset: r.get::<_, i64>(12)? as u64,
        line_count: r.get::<_, i64>(13)? as u64,
        failed_lines: r.get::<_, i64>(14)? as u64,
        first_error: r.get(15)?,
    })
}

pub fn all_files(conn: &Connection) -> CoreResult<Vec<FileRecord>> {
    let mut st = conn.prepare(&format!("SELECT {FILE_COLS} FROM files"))?;
    let rows = st.query_map([], file_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every `files` row of `source`.
pub fn files_of_source(conn: &Connection, source: FileSource) -> CoreResult<Vec<FileRecord>> {
    let mut st = conn.prepare_cached(&format!("SELECT {FILE_COLS} FROM files WHERE source=?1"))?;
    let rows = st.query_map([source_to_db(source)], file_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Stored `agy_meta` fingerprints of agy sessions (`id -> agy_meta`).
/// When `sids` is `Some`, only those sessions are queried; otherwise every agy session.
pub fn agy_meta_fingerprints(
    conn: &Connection,
    sids: Option<&BTreeSet<String>>,
) -> CoreResult<HashMap<String, Option<i64>>> {
    let mut out = HashMap::new();
    match sids {
        None => {
            let mut st = conn.prepare_cached(&format!(
                "SELECT id, agy_meta FROM sessions WHERE agent = '{SOURCE_AGY}'"
            ))?;
            for r in st.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
            })? {
                let (id, fp) = r?;
                out.insert(id, fp);
            }
        }
        Some(ids) => {
            let mut st = conn.prepare_cached(&format!(
                "SELECT agy_meta FROM sessions WHERE id = ?1 AND agent = '{SOURCE_AGY}'"
            ))?;
            for id in ids {
                if let Some(fp) = st
                    .query_row([id], |r| r.get::<_, Option<i64>>(0))
                    .optional()?
                {
                    out.insert(id.clone(), fp);
                }
            }
        }
    }
    Ok(out)
}

/// `files` rows of the sessions `sids`.
pub fn files_of_sessions<'a>(
    conn: &Connection,
    sids: impl IntoIterator<Item = &'a String>,
) -> CoreResult<Vec<FileRecord>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {FILE_COLS} FROM files WHERE session_id=?1"
    ))?;
    let mut out = Vec::new();
    for sid in sids {
        for r in st.query_map([sid], file_from_row)? {
            out.push(r?);
        }
    }
    Ok(out)
}

/// Files of a session, largest main file first, then agent files by path.
pub fn session_files(conn: &Connection, session_id: &str) -> CoreResult<Vec<FileRecord>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {FILE_COLS} FROM files WHERE session_id=?1 ORDER BY role, size DESC, path"
    ))?;
    let rows = st.query_map([session_id], file_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Projects with at least one listed session (non-empty, not superseded) started in or editing
/// them, most recently active first.
pub fn list_projects(conn: &Connection) -> CoreResult<Vec<ProjectSummary>> {
    let mut st = conn.prepare(
        "WITH rel AS (SELECT id AS session_id, project_id FROM sessions
                UNION SELECT session_id, project_id FROM session_projects)
         SELECT p.id, p.path, p.display_name, p.missing, count(s.id), max(s.last_active_ms)
         FROM projects p JOIN rel ON rel.project_id = p.id
            JOIN sessions s ON s.id = rel.session_id AND s.is_empty = 0 AND s.superseded = 0
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

/// Origin of a session: `forkedFrom`, else the latest earlier session sharing its `root_uuid` that
/// is not superseded (the Fork fallback of docs/spec.md「数据模型」; a superseded session is the
/// same conversation as the one carrying it on, never its origin). `s` is the alias of the session
/// row.
fn origin_expr(s: &str) -> String {
    format!(
        "coalesce({s}.fork_origin_id, (SELECT o2.id FROM sessions o2 WHERE {s}.root_uuid IS NOT NULL
            AND o2.root_uuid = {s}.root_uuid AND o2.id != {s}.id AND o2.created_ms < {s}.created_ms
            AND o2.superseded = 0
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
            (SELECT json_group_array(sp.project_id) FROM session_projects sp WHERE sp.session_id = b.id),
            (SELECT max(m.ts_ms) FROM messages m
                WHERE m.session_id = b.id AND m.agent_id = '' AND m.role = {MSG_PROMPT}),
            b.agent
         FROM b LEFT JOIN sessions o ON o.id = b.origin",
        origin = origin_expr("s")
    )
}

fn summary_from_row(r: &Row) -> rusqlite::Result<SessionSummary> {
    let last_active = r.get::<_, i64>(5)? as f64;
    let origin: Option<String> = r.get(15)?;
    let created_ms = r
        .get::<_, Option<i64>>(4)?
        .map_or(last_active, |v| v as f64);
    let agent: String = r.get(22)?;
    Ok(SessionSummary {
        id: r.get(0)?,
        agent: agent_from_db(&agent),
        project_id: r.get(1)?,
        title: r.get(2)?,
        title_source: title_source_from_db(r.get(3)?),
        created_ms,
        last_active_ms: last_active,
        // A Session without a prompt of its own yet: its start.
        last_prompt_ms: r
            .get::<_, Option<i64>>(21)?
            .map_or(created_ms, |v| v as f64),
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

/// Maximum distinct models accepted by `Drill::Model` (extras are dropped after deduplication).
const MAX_DRILL_MODELS: usize = 64;

/// Listed sessions (non-empty, not superseded) matching `q` (live filtering and `live` states are
/// applied by the engine).
///
/// Without a drill, the time range keeps sessions whose span overlaps it. With a drill, the
/// session (or a superseded session it carries on) must have a contributing billed request inside
/// the range, mirroring `stats::compute` (`stats::billed_tokens_sql`):
/// - `Day`: a billed request on that local day;
/// - `Model`: a billed request of any listed model (empty matches none).
pub fn list_sessions(conn: &Connection, q: &SessionQuery) -> CoreResult<Vec<SessionSummary>> {
    let mut conds = vec!["s.is_empty = 0 AND s.superseded = 0".to_owned()];
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
                let lo = start.max(from);
                let hi = (end - 1).min(to);
                if lo > hi {
                    conds.push("0".into());
                } else {
                    args.push(Value::Integer(lo));
                    args.push(Value::Integer(hi));
                    conds.push(drill_in_sessions_sql(&format!(
                        "{billed} AND m.ts_ms >= ? AND m.ts_ms <= ?",
                        billed = billed_tokens_sql()
                    )));
                }
            }
            None => conds.push("0".into()),
        },
        Some(Drill::Model { models }) => {
            let mut seen = HashSet::new();
            let unique: Vec<&str> = models
                .iter()
                .map(String::as_str)
                .filter(|m| !m.is_empty() && seen.insert(*m))
                .take(MAX_DRILL_MODELS)
                .collect();
            if unique.is_empty() {
                conds.push("0".into());
            } else {
                let marks = vec!["?"; unique.len()].join(",");
                args.push(Value::Text(stats::UNKNOWN.to_owned()));
                args.extend(unique.into_iter().map(|m| Value::Text(m.to_owned())));
                let range = in_range("m.ts_ms", &mut args);
                conds.push(drill_in_sessions_sql(&format!(
                    "coalesce(m.model, ?) IN ({marks}) AND {billed}{range}",
                    billed = billed_tokens_sql()
                )));
            }
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

/// SQL predicate matching listed Claude Code sessions that either directly own a message
/// satisfying `where_sql` (over `messages m`) or carry on a superseded session (through any
/// `continued_in_id` chain) that does.
fn drill_in_sessions_sql(where_sql: &str) -> String {
    format!(
        "s.id IN (
            WITH RECURSIVE climb(sid) AS (
                SELECT DISTINCT m.session_id
                FROM messages m
                WHERE {where_sql}
                  AND m.session_id IN (SELECT id FROM sessions WHERE is_empty = 0 AND agent = '{SOURCE_CLAUDE}')
                UNION
                SELECT cur.continued_in_id
                FROM climb c
                JOIN sessions cur ON cur.id = c.sid
                WHERE cur.superseded = 1 AND cur.continued_in_id IS NOT NULL
            )
            SELECT sid FROM climb
        )"
    )
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
}

/// Summaries of the sessions `ids`, by id (one query; missing ids are absent).
pub fn session_summaries(
    conn: &Connection,
    ids: &[&str],
) -> CoreResult<HashMap<String, SessionSummary>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = summary_select(&format!("s.id IN ({})", vec!["?"; ids.len()].join(",")));
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(ids), summary_from_row)?;
    rows.map(|r| r.map(|s| (s.id.clone(), s)).map_err(Into::into))
        .collect()
}

pub fn session_detail(conn: &Connection, id: &str) -> CoreResult<Option<SessionDetailRow>> {
    let sql = summary_select("s.id = ?1");
    let Some(summary) = conn.query_row(&sql, [id], summary_from_row).optional()? else {
        return Ok(None);
    };
    let row = conn.query_row(
        "SELECT s.cwd, coalesce(p.path, s.project_id), coalesce(p.missing, 1), s.duration_ms,
            s.models_json, s.versions_json, s.main_in_tok, s.main_out_tok, s.main_cr_tok,
            s.main_cc_tok
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
            })
        },
    )?;
    Ok(Some(row))
}

/// Cost per model of a Claude Code session (main + subagents), most expensive first; empty for agy.
pub fn model_costs(conn: &Connection, session_id: &str) -> CoreResult<Vec<ModelCost>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT coalesce(model, ?2), coalesce(sum(cost), 0), count(*) FILTER (WHERE cost IS NULL)
         FROM messages
         WHERE session_id = ?1
           AND EXISTS (SELECT 1 FROM sessions s WHERE s.id = messages.session_id AND s.agent = '{SOURCE_CLAUDE}')
           AND {billed}
         GROUP BY 1 ORDER BY 2 DESC, 1",
        billed = billed_tokens_sql()
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
/// Forks of a session `id` carries on count as its own (their Origin Session opens as `id`).
pub fn fork_children(conn: &Connection, id: &str) -> CoreResult<Vec<ForkChild>> {
    let mut ids = vec![id.to_owned()];
    ids.extend(superseded_into(conn, id)?);
    let marks = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT c.id, c.title, coalesce(c.created_ms, c.last_active_ms) FROM sessions c
         WHERE c.is_empty = 0 AND c.superseded = 0 AND c.id NOT IN ({marks})
            AND (c.fork_origin_id IN ({marks})
                OR (c.fork_origin_id IS NULL AND {origin} IN ({marks})))
         ORDER BY coalesce(c.created_ms, c.last_active_ms)",
        origin = origin_expr("c")
    );
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(ids.iter().chain(&ids).chain(&ids)), |r| {
        Ok(ForkChild {
            session_id: r.get(0)?,
            title: r.get(1)?,
            created_ms: r.get::<_, i64>(2)? as f64,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The listed session a superseded one is carried on in (following `continued-in` through
/// sessions superseded in turn); `None` when `id` is not superseded.
pub fn superseded_by(conn: &Connection, id: &str) -> CoreResult<Option<String>> {
    Ok(successor(conn, id)?.map(|(to, _)| to))
}

/// [`superseded_by`] with the number of `continued-in` hops to it.
fn successor(conn: &Connection, id: &str) -> CoreResult<Option<(String, usize)>> {
    let mut st = conn
        .prepare_cached("SELECT continued_in_id FROM sessions WHERE id = ?1 AND superseded = 1")?;
    let mut cur = id.to_owned();
    let mut seen = HashSet::from([cur.clone()]);
    while let Some(next) = st
        .query_row([&cur], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
    {
        if !seen.insert(next.clone()) {
            break;
        }
        cur = next;
    }
    Ok((cur != id).then(|| (cur, seen.len() - 1)))
}

/// The sessions `id` carries on: superseded by it directly or through others, nearest first
/// (fewest `continued-in` hops, then by id: the order of [`adoptions`]).
pub fn superseded_into(conn: &Connection, id: &str) -> CoreResult<Vec<String>> {
    let mut st = conn
        .prepare_cached("SELECT id FROM sessions WHERE continued_in_id = ?1 AND superseded = 1")?;
    let mut out: Vec<String> = Vec::new();
    let mut level = vec![id.to_owned()];
    while !level.is_empty() {
        let mut next: Vec<String> = Vec::new();
        for cur in &level {
            for r in st.query_map([cur], |r| r.get::<_, String>(0))? {
                let r = r?;
                if r != id && !out.contains(&r) && !next.contains(&r) {
                    next.push(r);
                }
            }
        }
        next.sort();
        out.extend(next.iter().cloned());
        level = next;
    }
    Ok(out)
}

/// Every superseded session with the listed session carrying it on (as [`superseded_by`]),
/// nearest first (fewest `continued-in` hops, then by id): the order `scan::adopt` gives
/// precedence by.
pub fn adoptions(conn: &Connection) -> CoreResult<Vec<(String, String)>> {
    let mut st = conn.prepare_cached("SELECT id FROM sessions WHERE superseded = 1")?;
    let ids: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    for id in ids {
        if let Some((to, hops)) = successor(conn, &id)? {
            out.push((hops, id, to));
        }
    }
    out.sort();
    Ok(out.into_iter().map(|(_, id, to)| (id, to)).collect())
}

/// Sessions outside `scope` whose `files` rows hold one of `paths` (a file adopted by, or still
/// recorded under, a session the scan did not reach).
pub fn owners_outside<'a>(
    conn: &Connection,
    paths: impl IntoIterator<Item = &'a str>,
    scope: &BTreeSet<String>,
) -> CoreResult<BTreeSet<String>> {
    let mut st = conn.prepare_cached("SELECT session_id FROM files WHERE path = ?1")?;
    let mut out = BTreeSet::new();
    for p in paths {
        if let Some(sid) = st
            .query_row([p], |r| r.get::<_, String>(0))
            .optional()?
            .filter(|s| !scope.contains(s))
        {
            out.insert(sid);
        }
    }
    Ok(out)
}

/// `sids` and every session linked to one of them by `continued-in`, either way and through
/// chains (superseded or not): their agent files may move between them.
pub fn with_continued(conn: &Connection, sids: &BTreeSet<String>) -> CoreResult<BTreeSet<String>> {
    let mut st = conn.prepare_cached(
        "SELECT id, continued_in_id FROM sessions WHERE continued_in_id IS NOT NULL",
    )?;
    let edges: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut out = sids.clone();
    loop {
        let before = out.len();
        for (a, b) in &edges {
            if out.contains(a) || out.contains(b) {
                out.insert(a.clone());
                out.insert(b.clone());
            }
        }
        if out.len() == before {
            return Ok(out);
        }
    }
}

/// Those of `ids` that are superseded.
pub fn superseded_among<'a>(
    conn: &Connection,
    ids: impl IntoIterator<Item = &'a String>,
) -> CoreResult<HashSet<String>> {
    let mut st = conn.prepare_cached("SELECT 1 FROM sessions WHERE id = ?1 AND superseded = 1")?;
    let mut out = HashSet::new();
    for id in ids {
        if st.exists([id])? {
            out.insert(id.clone());
        }
    }
    Ok(out)
}

/// Fork origin of a session (same rule as the summaries).
pub fn fork_origin(conn: &Connection, id: &str) -> CoreResult<Option<ForkOrigin>> {
    let sql = summary_select("s.id = ?1");
    Ok(conn
        .query_row(&sql, [id], summary_from_row)
        .optional()?
        .and_then(|s| s.fork_origin))
}

/// Per-agent figures from `messages` / `tool_calls`.
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

/// Number of listed sessions (non-empty, not superseded).
pub fn session_count(conn: &Connection) -> CoreResult<u32> {
    Ok(conn.query_row(
        "SELECT count(*) FROM sessions WHERE is_empty = 0 AND superseded = 0",
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

/// Those of `sids` that have `msg_text` rows.
pub fn sessions_with_text<'a>(
    conn: &Connection,
    sids: impl IntoIterator<Item = &'a String>,
) -> CoreResult<HashSet<String>> {
    let mut out = HashSet::new();
    for sid in sids {
        if super::writer::has_text_rows(conn, sid)? {
            out.insert(sid.clone());
        }
    }
    Ok(out)
}
