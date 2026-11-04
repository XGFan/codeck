//! FTS5 trigram search with LIKE fallback for terms shorter than 3 characters.
//!
//! - Positive terms of ≥ 3 characters become an FTS5 phrase expression (`Fts`); shorter terms and
//!   exclusions become `instr(lower(body), lower(?))` predicates on the joined `msg_text`
//!   (`Mixed`). Without any FTS-able positive term the scan runs over `msg_text` alone (`Like`),
//!   with FTS-able exclusions as `id NOT IN (… MATCH …)` (`Mixed`, A13).
//! - The LIKE fallback is ASCII-case-insensitive only (SQLite `lower()`); trigram FTS folds case.
//! - Sessions are ordered by their latest hit; each keeps its newest `hits_per_session` hits.
//! - Snippets are a ±60-character window built in Rust (FTS5 `snippet()` with a rowid list costs
//!   ~0.3 ms per row); `snippet()` is used only for rows where the ASCII-case-insensitive window
//!   finds no term because trigram FTS folded a non-ASCII case.
//! - `SearchHit.node_id` is the entry uuid of the indexed fragment; `resolve_jump` maps it to the
//!   display node (A3).

use std::collections::HashMap;
use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use super::query::{fts_phrase, is_fts_term, parse_query};
use super::snippet::{self, HIT_END, HIT_START};
use crate::error::CoreResult;
use crate::index::reader;
use crate::model::{SearchGroup, SearchHit, SearchMode, SearchRequest, SearchResponse, SearchRole};

/// Tokens of context passed to `snippet()` (trigram tokens are characters, so 16 would leave
/// about 16 characters; 64 is FTS5's maximum).
const SNIPPET_TOKENS: u32 = 64;
const DEFAULT_MAX_SESSIONS: u32 = 50;
const DEFAULT_HITS_PER_SESSION: u32 = 5;

/// Searches user input, Claude replies and tool input (tool output is not indexed, ADR-0004).
pub fn run_fts(conn: &Connection, r: &SearchRequest) -> CoreResult<SearchResponse> {
    run(conn, r, false)
}

/// [`run_fts`]; `force_like` evaluates every term with the LIKE fallback (tests compare modes).
pub fn run(conn: &Connection, r: &SearchRequest, force_like: bool) -> CoreResult<SearchResponse> {
    let started = Instant::now();
    let q = parse_query(&r.query)?;
    let fts = |t: &&String| !force_like && is_fts_term(t);
    let (fts_inc, like_inc): (Vec<&String>, Vec<&String>) = q.include.iter().partition(fts);
    let (fts_exc, like_exc): (Vec<&String>, Vec<&String>) = q.exclude.iter().partition(fts);
    let mode = if !fts_inc.is_empty() {
        if like_inc.is_empty() && like_exc.is_empty() {
            SearchMode::Fts
        } else {
            SearchMode::Mixed
        }
    } else if !fts_exc.is_empty() {
        SearchMode::Mixed
    } else {
        SearchMode::Like
    };
    let mut resp = SearchResponse {
        groups: Vec::new(),
        total_sessions: 0,
        total_hits: 0,
        mode,
        elapsed_ms: 0.0,
        index_complete: reader::text_ready(conn)?,
    };
    let mut args: Vec<Value> = Vec::new();
    let mut conds: Vec<String> = Vec::new();
    let match_expr = (!fts_inc.is_empty()).then(|| {
        let inc: Vec<String> = fts_inc.iter().map(|t| fts_phrase(t)).collect();
        let mut e = format!("({})", inc.join(" AND "));
        if !fts_exc.is_empty() {
            let exc: Vec<String> = fts_exc.iter().map(|t| fts_phrase(t)).collect();
            e = format!("{e} NOT ({})", exc.join(" OR "));
        }
        e
    });
    let from = match &match_expr {
        Some(e) => {
            conds.push("msg_fts MATCH ?".into());
            args.push(Value::Text(e.clone()));
            "msg_fts JOIN msg_text t ON t.id = msg_fts.rowid"
        }
        None => {
            if !fts_exc.is_empty() {
                let exc: Vec<String> = fts_exc.iter().map(|t| fts_phrase(t)).collect();
                conds.push("t.id NOT IN (SELECT rowid FROM msg_fts WHERE msg_fts MATCH ?)".into());
                args.push(Value::Text(exc.join(" OR ")));
            }
            "msg_text t"
        }
    };
    for t in &like_inc {
        conds.push("instr(lower(t.body), lower(?)) > 0".into());
        args.push(Value::Text((*t).clone()));
    }
    for t in &like_exc {
        conds.push("instr(lower(t.body), lower(?)) = 0".into());
        args.push(Value::Text((*t).clone()));
    }
    if let Some(p) = &r.project_id {
        conds.push(reader::project_match(std::slice::from_ref(p), &mut args));
    }
    let max_sessions = nonzero(r.max_sessions, DEFAULT_MAX_SESSIONS);
    let per_session = nonzero(r.hits_per_session, DEFAULT_HITS_PER_SESSION);
    args.push(Value::Integer(i64::from(max_sessions)));
    args.push(Value::Integer(i64::from(per_session)));
    let sql = format!(
        "WITH h AS MATERIALIZED (
            SELECT t.id, t.session_id, coalesce(t.ts_ms, 0) AS ts
            FROM {from} JOIN sessions s ON s.id = t.session_id AND s.is_empty = 0
            WHERE {conds}),
         g AS MATERIALIZED (SELECT session_id, max(ts) AS last, count(*) AS n FROM h GROUP BY session_id),
         top AS (SELECT session_id, last, n FROM g ORDER BY last DESC, session_id LIMIT ?),
         ranked AS (SELECT h.id, h.session_id, h.ts,
            row_number() OVER (PARTITION BY h.session_id ORDER BY h.ts DESC, h.id DESC) AS rn
            FROM h JOIN top USING (session_id))
         SELECT ranked.id, ranked.session_id, top.n,
            (SELECT count(*) FROM g), (SELECT coalesce(sum(n), 0) FROM g)
         FROM ranked JOIN top USING (session_id)
         WHERE rn <= ?
         ORDER BY top.last DESC, top.session_id, ranked.ts DESC, ranked.id DESC",
        conds = conds.join(" AND ")
    );
    // (row id, session id, session hit count) in output order.
    let mut picked: Vec<(i64, String, u32)> = Vec::new();
    {
        let mut st = conn.prepare(&sql)?;
        let mut rows = st.query(params_from_iter(args))?;
        while let Some(row) = rows.next()? {
            resp.total_sessions = row.get(3)?;
            resp.total_hits = row.get(4)?;
            picked.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
    }
    if picked.is_empty() {
        resp.elapsed_ms = started.elapsed().as_secs_f64() * 1e3;
        return Ok(resp);
    }

    let ids = picked
        .iter()
        .map(|p| p.0.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut details: HashMap<i64, SearchHit> = HashMap::new();
    // Rows whose ASCII-case-insensitive window found no term (FTS folded a non-ASCII case).
    let mut unmatched: Vec<i64> = Vec::new();
    {
        let mut st = conn.prepare(&format!(
            "SELECT m.id, m.node_uuid, m.agent_id, m.tool_use_id, m.role, m.ts_ms, m.on_main_line, m.body,
                a.agent_type
             FROM msg_text m
             LEFT JOIN subagents a ON a.session_id = m.session_id AND a.agent_id = m.agent_id
             WHERE m.id IN ({ids})"
        ))?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let body: String = row.get(7)?;
            let snippet = snippet::window(&body, &q.include);
            if !snippet.iter().any(|p| p.hit) {
                unmatched.push(id);
            }
            details.insert(
                id,
                SearchHit {
                    node_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    agent_type: row.get(8)?,
                    tool_use_id: row.get(3)?,
                    role: role_from_db(row.get(4)?),
                    timestamp_ms: row.get::<_, Option<i64>>(5)?.map(|v| v as f64),
                    on_main_line: row.get(6)?,
                    snippet,
                },
            );
        }
    }
    if let (Some(e), false) = (&match_expr, unmatched.is_empty()) {
        let ids = unmatched
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut st = conn.prepare(&format!(
            "SELECT rowid, snippet(msg_fts, 0, ?2, ?3, '…', {SNIPPET_TOKENS}) FROM msg_fts
             WHERE msg_fts MATCH ?1 AND rowid IN ({ids})"
        ))?;
        let mut rows = st.query((e, HIT_START.to_string(), HIT_END.to_string()))?;
        while let Some(row) = rows.next()? {
            if let Some(h) = details.get_mut(&row.get::<_, i64>(0)?) {
                h.snippet = snippet::parse_marked(&row.get::<_, String>(1)?);
            }
        }
    }

    for (id, sid, n) in picked {
        let Some(hit) = details.remove(&id) else {
            continue;
        };
        if resp.groups.last().is_none_or(|g| g.session.id != sid) {
            let Some(row) = reader::session_detail(conn, &sid)? else {
                continue;
            };
            resp.groups.push(SearchGroup {
                session: row.summary,
                hit_count: n,
                hits: Vec::new(),
            });
        }
        if let Some(g) = resp.groups.last_mut().filter(|g| g.session.id == sid) {
            g.hits.push(hit);
        }
    }
    resp.elapsed_ms = started.elapsed().as_secs_f64() * 1e3;
    Ok(resp)
}

fn nonzero(v: u32, default: u32) -> u32 {
    if v == 0 { default } else { v }
}

fn role_from_db(v: i64) -> SearchRole {
    match v {
        0 => SearchRole::User,
        1 => SearchRole::Assistant,
        _ => SearchRole::ToolInput,
    }
}
