//! Metadata upserts and incremental per-file processing (plan §4, A7).
//!
//! Row derivation ([`extract_rows`]) is pure and runs in parallel; the `write_*` / `delete_*`
//! functions run inside the single writer transaction. Row ownership (A7): `messages` merge by
//! key with the max of each token field, `tool_calls` / `persisted_outputs` keep the first row,
//! and anything that cannot be merged (a removed or rewritten file) goes through a session-level
//! reparse (delete every row of the session, then re-ingest all its files).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

use super::schema::{
    DIAG_BLOCK, DIAG_ENTRY, DIAG_SYSTEM_SUBTYPE, DIAG_TOOL, MSG_ASSISTANT, MSG_PROMPT, ROLE_MAIN,
    ROLE_SUBAGENT, ROLE_WORKFLOW_SUBAGENT,
};
use crate::error::CoreResult;
use crate::known;
use crate::model::{FileRole, TitleSource, TokenTotals};
use crate::parse::EntryRecord;
use crate::project;
use crate::raw::{RawEntry, decode_str, lenient};

pub fn role_to_db(role: FileRole) -> i64 {
    match role {
        FileRole::Main => ROLE_MAIN,
        FileRole::Subagent => ROLE_SUBAGENT,
        FileRole::WorkflowSubagent => ROLE_WORKFLOW_SUBAGENT,
    }
}

pub fn role_from_db(v: i64) -> FileRole {
    match v {
        ROLE_SUBAGENT => FileRole::Subagent,
        ROLE_WORKFLOW_SUBAGENT => FileRole::WorkflowSubagent,
        _ => FileRole::Main,
    }
}

pub fn title_source_to_db(t: TitleSource) -> i64 {
    match t {
        TitleSource::Custom => 0,
        TitleSource::Ai => 1,
        TitleSource::FirstPrompt => 2,
        TitleSource::Untitled => 3,
    }
}

pub fn title_source_from_db(v: i64) -> TitleSource {
    match v {
        0 => TitleSource::Custom,
        1 => TitleSource::Ai,
        2 => TitleSource::FirstPrompt,
        _ => TitleSource::Untitled,
    }
}

/// One `messages` row: a human/command prompt (key = uuid) or an assistant message (key =
/// `message.id`, falling back to the uuid).
#[derive(Clone, Debug, PartialEq)]
pub struct MessageRow {
    pub key: String,
    pub role: i64,
    pub ts_ms: i64,
    pub model: Option<String>,
    /// input, output, cache read, cache creation (all), cache creation 1-hour part.
    pub tokens: [i64; 5],
    /// `usage.speed == "fast"`.
    pub fast: bool,
    /// `costUSD` Claude Code logged on the entry (older versions).
    pub logged_cost: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallRow {
    pub tool_use_id: String,
    pub name: String,
    pub ts_ms: Option<i64>,
    /// File an edit tool ([`project::EDIT_TOOLS`]) wrote.
    pub file_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersistedRow {
    pub file_name: String,
    pub tool_use_id: String,
}

/// Rows derived from a run of records of one file.
#[derive(Clone, Debug, Default)]
pub struct FileRows {
    pub messages: Vec<MessageRow>,
    pub tool_calls: Vec<ToolCallRow>,
    /// tool_use ids whose result has `is_error: true`.
    pub tool_errors: Vec<String>,
    pub persisted: Vec<PersistedRow>,
    /// (category, name, version) → count; unknown names only.
    pub diag: HashMap<(i64, String, String), i64>,
}

/// Derives the metadata rows of `records` (light parse, A6).
pub fn extract_rows<'a>(records: impl IntoIterator<Item = &'a EntryRecord>) -> FileRows {
    let mut rows = FileRows::default();
    let mut msg_index: HashMap<String, usize> = HashMap::new();
    for rec in records {
        let e = &rec.entry;
        let ts = e.timestamp_ms().map(|t| t as i64);
        let version = e.version.clone().unwrap_or_default();
        let ty = e.entry_type();
        if !ty.is_empty() && !known::is_known_entry_type(ty) {
            *rows
                .diag
                .entry((DIAG_ENTRY, ty.to_owned(), version.clone()))
                .or_default() += 1;
        }
        if ty == "system"
            && let Some(sub) = e.subtype.as_deref()
            && !known::is_known_system_subtype(sub)
        {
            *rows
                .diag
                .entry((DIAG_SYSTEM_SUBTYPE, sub.to_owned(), version.clone()))
                .or_default() += 1;
        }
        for b in e.blocks() {
            let bt = b.block_type();
            if !bt.is_empty() && !known::is_known_block_type(bt) {
                *rows
                    .diag
                    .entry((DIAG_BLOCK, bt.to_owned(), version.clone()))
                    .or_default() += 1;
            }
        }
        match ty {
            "assistant" => {
                let Some(msg) = e.message.as_ref() else {
                    continue;
                };
                for b in e.blocks() {
                    if b.block_type() != "tool_use" {
                        continue;
                    }
                    let (Some(id), Some(name)) = (b.id.as_ref(), b.name.as_ref()) else {
                        continue;
                    };
                    if !known::is_known_tool(name) {
                        *rows
                            .diag
                            .entry((DIAG_TOOL, name.clone(), version.clone()))
                            .or_default() += 1;
                    }
                    rows.tool_calls.push(ToolCallRow {
                        tool_use_id: id.clone(),
                        name: name.clone(),
                        ts_ms: ts,
                        file_path: edit_target(name, b.input.as_deref()),
                    });
                }
                let Some(ts) = ts else { continue };
                let Some(key) = msg.id.clone().or_else(|| e.uuid.clone()) else {
                    continue;
                };
                let u = msg.usage.clone().unwrap_or_default();
                let tokens = [
                    u.input_tokens,
                    u.output_tokens,
                    u.cache_read_input_tokens,
                    u.cache_creation_input_tokens,
                    u.cache_creation
                        .as_ref()
                        .and_then(|c| c.ephemeral_1h_input_tokens),
                ]
                .map(|v| v.unwrap_or(0.0) as i64);
                let row = MessageRow {
                    key: key.clone(),
                    role: MSG_ASSISTANT,
                    ts_ms: ts,
                    model: msg.model.clone(),
                    tokens,
                    fast: u.speed.as_deref() == Some("fast"),
                    logged_cost: e.cost_usd,
                };
                match msg_index.get(&key) {
                    Some(&i) => merge_message(&mut rows.messages[i], &row),
                    None => {
                        msg_index.insert(key, rows.messages.len());
                        rows.messages.push(row);
                    }
                }
            }
            "user" => {
                collect_tool_results(e, &mut rows);
                if let (Some(ts), Some(uuid)) = (ts, e.uuid.as_ref())
                    && is_human_prompt(e)
                    && !msg_index.contains_key(uuid)
                {
                    msg_index.insert(uuid.clone(), rows.messages.len());
                    rows.messages.push(MessageRow {
                        key: uuid.clone(),
                        role: MSG_PROMPT,
                        ts_ms: ts,
                        model: None,
                        tokens: [0; 5],
                        fast: false,
                        logged_cost: None,
                    });
                }
            }
            _ => {}
        }
    }
    rows
}

fn merge_message(into: &mut MessageRow, other: &MessageRow) {
    for (a, b) in into.tokens.iter_mut().zip(other.tokens) {
        *a = (*a).max(b);
    }
    if into.model.is_none() {
        into.model = other.model.clone();
    }
    into.fast |= other.fast;
    into.logged_cost = max_cost(into.logged_cost, other.logged_cost);
}

fn max_cost(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// `cv_cost(model, in, out, cr, cc, cc1h, fast, logged_cost)` over `messages` columns: the
/// message's USD cost, NULL when its model has no price (see [`crate::pricing`]). Registered on
/// the writer connection by [`crate::index::Index::open`].
pub const COST_EXPR: &str =
    "cv_cost(model, in_tok, out_tok, cr_tok, cc_tok, cc1h_tok, fast, logged_cost)";

/// Sessions other than `session_id` holding a copy of one of its assistant messages (a resumed or
/// forked session repeats the earlier conversation in its own file).
pub fn copy_neighbors(conn: &Connection, session_id: &str) -> CoreResult<Vec<String>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT DISTINCT o.session_id FROM messages m
         JOIN messages o ON o.key = m.key AND o.role = {MSG_ASSISTANT} AND o.session_id != m.session_id
         WHERE m.session_id = ?1 AND m.role = {MSG_ASSISTANT}"
    ))?;
    let rows = st.query_map([session_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Billing (ccusage's dedupe): every assistant message is billed once, in the copy whose session
/// started first (then by session id, main before Subagents); `billed` marks that copy. Then the
/// sessions' token and cost columns are re-summed from their billed messages. `sessions` limits
/// both steps (it must include every session sharing a message with a changed one); `None` = all.
pub fn rebill(conn: &Connection, sessions: Option<&HashSet<String>>) -> CoreResult<()> {
    let scope = match sessions {
        Some(ids) => {
            conn.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS temp_rebill(id TEXT PRIMARY KEY); DELETE FROM temp_rebill;",
            )?;
            let mut st =
                conn.prepare_cached("INSERT OR IGNORE INTO temp_rebill(id) VALUES (?1)")?;
            for id in ids {
                st.execute([id])?;
            }
            Some(())
        }
        None => None,
    };
    let (msg_scope, session_scope) = match scope {
        Some(()) => (
            " AND session_id IN (SELECT id FROM temp_rebill)",
            " WHERE id IN (SELECT id FROM temp_rebill)",
        ),
        None => ("", ""),
    };
    conn.execute(
        &format!(
            "UPDATE messages SET billed = NOT EXISTS (
                SELECT 1 FROM messages o JOIN sessions so ON so.id = o.session_id
                WHERE o.key = messages.key AND o.role = {MSG_ASSISTANT}
                    AND (coalesce(so.created_ms, so.last_active_ms), o.session_id, o.agent_id) <
                        ((SELECT coalesce(s.created_ms, s.last_active_ms) FROM sessions s
                            WHERE s.id = messages.session_id), messages.session_id, messages.agent_id))
             WHERE role = {MSG_ASSISTANT}{msg_scope}"
        ),
        [],
    )?;
    conn.execute(
        &format!(
            "UPDATE sessions SET (in_tok, out_tok, cr_tok, cc_tok, main_in_tok, main_out_tok, main_cr_tok,
                main_cc_tok, cost, unpriced) = (
                SELECT coalesce(sum(in_tok), 0), coalesce(sum(out_tok), 0), coalesce(sum(cr_tok), 0),
                    coalesce(sum(cc_tok), 0),
                    coalesce(sum(in_tok) FILTER (WHERE agent_id = ''), 0),
                    coalesce(sum(out_tok) FILTER (WHERE agent_id = ''), 0),
                    coalesce(sum(cr_tok) FILTER (WHERE agent_id = ''), 0),
                    coalesce(sum(cc_tok) FILTER (WHERE agent_id = ''), 0),
                    coalesce(sum(cost), 0), count(*) FILTER (WHERE cost IS NULL)
                FROM messages m WHERE m.session_id = sessions.id AND m.role = {MSG_ASSISTANT}
                    AND m.billed = 1){session_scope}"
        ),
        [],
    )?;
    if scope.is_some() {
        conn.execute("DELETE FROM temp_rebill", [])?;
    }
    Ok(())
}

/// Recomputes every message's cost with the current price table, then every session's total.
pub fn reprice_all(conn: &Connection) -> CoreResult<()> {
    conn.execute(
        &format!("UPDATE messages SET cost = {COST_EXPR} WHERE role = 1"),
        [],
    )?;
    rebill(conn, None)
}

/// Human or command prompt (the assembly classifier, so `messages` agrees with `message_count`).
pub fn is_human_prompt(e: &RawEntry) -> bool {
    crate::assemble::prompt::is_human_prompt(e)
}

const PERSISTED_TAG: &str = "<persisted-output>";
const SAVED_TO: &str = "Full output saved to: ";

fn collect_tool_results(e: &RawEntry, rows: &mut FileRows) {
    let results: Vec<_> = e
        .blocks()
        .iter()
        .filter(|b| b.block_type() == "tool_result")
        .collect();
    if results.is_empty() {
        return;
    }
    let tur_path = e
        .tool_use_result
        .as_ref()
        .filter(|r| r.get().contains("persistedOutputPath"))
        .and_then(|_| e.tool_use_result_fields())
        .and_then(|f| f.persisted_output_path);
    for b in &results {
        let Some(id) = b.tool_use_id.as_ref() else {
            continue;
        };
        if b.is_error == Some(true) {
            rows.tool_errors.push(id.clone());
        }
        let path = if results.len() == 1 {
            tur_path.clone()
        } else {
            None
        }
        .or_else(|| persisted_path_from_content(b.content.as_deref()));
        if let Some(name) = path.as_deref().and_then(basename) {
            rows.persisted.push(PersistedRow {
                file_name: name,
                tool_use_id: id.clone(),
            });
        }
    }
}

#[derive(Deserialize)]
struct TextPart {
    #[serde(default, deserialize_with = "lenient::string")]
    text: Option<String>,
}

/// `Full output saved to: <path>` of a `<persisted-output>` result (string or text blocks).
fn persisted_path_from_content(raw: Option<&serde_json::value::RawValue>) -> Option<String> {
    let raw = raw?;
    if !raw.get().contains(PERSISTED_TAG) {
        return None;
    }
    let text = if raw.get().starts_with('"') {
        decode_str(Some(raw))?
    } else {
        let parts: Vec<TextPart> = serde_json::from_str(raw.get()).ok()?;
        parts.into_iter().find_map(|p| p.text)?
    };
    let t = text.trim_start();
    if !t.starts_with(PERSISTED_TAG) {
        return None;
    }
    let rest = &t[t.find(SAVED_TO)? + SAVED_TO.len()..];
    Some(rest.lines().next()?.trim().to_owned())
}

#[derive(Deserialize)]
struct EditInput {
    #[serde(default, deserialize_with = "lenient::string")]
    file_path: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    notebook_path: Option<String>,
}

/// File written by an edit tool call; `None` for other tools.
fn edit_target(name: &str, input: Option<&serde_json::value::RawValue>) -> Option<String> {
    if !project::EDIT_TOOLS.contains(&name) {
        return None;
    }
    let i: EditInput = serde_json::from_str(input?.get()).ok()?;
    i.file_path.or(i.notebook_path).filter(|p| !p.is_empty())
}

/// Basename of an (absolute) persisted-output path; never opened as given (F8).
fn basename(p: &str) -> Option<String> {
    let name = Path::new(p).file_name()?.to_str()?;
    (!name.is_empty() && name != "..").then(|| name.to_owned())
}

/// `files` row values written after an ingest.
#[derive(Clone, Debug)]
pub struct FileWrite<'a> {
    pub path: &'a str,
    pub dev: u64,
    pub ino: u64,
    pub role: FileRole,
    pub session_id: &'a str,
    pub agent_id: Option<&'a str>,
    pub size: u64,
    pub mtime_ns: i64,
    pub head: &'a [u8],
    pub parsed_offset: u64,
    pub line_count: u64,
    pub failed_lines: u64,
    pub first_error: Option<&'a str>,
}

/// Inserts or updates a `files` row by path and returns its id. `text_offset` is never touched
/// here (phase 2 owns it).
pub fn write_file(conn: &Connection, f: &FileWrite) -> CoreResult<i64> {
    let id = conn
        .prepare_cached(
            "INSERT INTO files(path, dev, ino, role, session_id, agent_id, size, mtime_ns, head,
                parsed_offset, line_count, failed_lines, first_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(path) DO UPDATE SET dev=excluded.dev, ino=excluded.ino, role=excluded.role,
                session_id=excluded.session_id, agent_id=excluded.agent_id, size=excluded.size,
                mtime_ns=excluded.mtime_ns, head=excluded.head, parsed_offset=excluded.parsed_offset,
                line_count=excluded.line_count, failed_lines=excluded.failed_lines,
                first_error=excluded.first_error
             RETURNING id",
        )?
        .query_row(
            params![
                f.path,
                f.dev as i64,
                f.ino as i64,
                role_to_db(f.role),
                f.session_id,
                f.agent_id,
                f.size as i64,
                f.mtime_ns,
                f.head,
                f.parsed_offset as i64,
                f.line_count as i64,
                f.failed_lines as i64,
                f.first_error,
            ],
            |r| r.get(0),
        )?;
    Ok(id)
}

/// Stat-only change (e.g. a partial trailing line was appended): size and mtime.
pub fn write_file_stat(conn: &Connection, id: i64, size: u64, mtime_ns: i64) -> CoreResult<()> {
    conn.prepare_cached("UPDATE files SET size=?2, mtime_ns=?3 WHERE id=?1")?
        .execute(params![id, size as i64, mtime_ns])?;
    Ok(())
}

/// Inserts the rows of one file chunk (A7 merge rules).
pub fn write_rows(
    conn: &Connection,
    file_id: i64,
    session_id: &str,
    agent_id: &str,
    rows: &FileRows,
) -> CoreResult<()> {
    let mut st = conn.prepare_cached(
        "INSERT INTO messages(session_id, agent_id, key, role, ts_ms, model, in_tok, out_tok, cr_tok, cc_tok,
            cc1h_tok, fast, logged_cost)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(session_id, agent_id, key) DO UPDATE SET
            in_tok=max(in_tok, excluded.in_tok), out_tok=max(out_tok, excluded.out_tok),
            cr_tok=max(cr_tok, excluded.cr_tok), cc_tok=max(cc_tok, excluded.cc_tok),
            cc1h_tok=max(cc1h_tok, excluded.cc1h_tok), fast=max(fast, excluded.fast),
            logged_cost=max(coalesce(logged_cost, excluded.logged_cost), coalesce(excluded.logged_cost, logged_cost)),
            model=coalesce(model, excluded.model)",
    )?;
    // Priced after the upsert, on the tokens merged with earlier fragments.
    let mut price = conn.prepare_cached(&format!(
        "UPDATE messages SET cost = {COST_EXPR} WHERE session_id = ?1 AND agent_id = ?2 AND key = ?3"
    ))?;
    for m in &rows.messages {
        st.execute(params![
            session_id,
            agent_id,
            m.key,
            m.role,
            m.ts_ms,
            m.model,
            m.tokens[0],
            m.tokens[1],
            m.tokens[2],
            m.tokens[3],
            m.tokens[4],
            m.fast,
            m.logged_cost,
        ])?;
        if m.role == MSG_ASSISTANT {
            price.execute(params![session_id, agent_id, m.key])?;
        }
    }
    let mut st = conn.prepare_cached(
        "INSERT INTO tool_calls(session_id, agent_id, tool_use_id, name, ts_ms, file_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT DO NOTHING",
    )?;
    for t in &rows.tool_calls {
        st.execute(params![
            session_id,
            agent_id,
            t.tool_use_id,
            t.name,
            t.ts_ms,
            t.file_path
        ])?;
    }
    let mut st = conn.prepare_cached(
        "UPDATE tool_calls SET is_error=1 WHERE session_id=?1 AND tool_use_id=?2",
    )?;
    for id in &rows.tool_errors {
        st.execute(params![session_id, id])?;
    }
    let mut st = conn.prepare_cached(
        "INSERT INTO persisted_outputs(session_id, file_name, tool_use_id, agent_id)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
    )?;
    let agent = (!agent_id.is_empty()).then_some(agent_id);
    for p in &rows.persisted {
        st.execute(params![session_id, p.file_name, p.tool_use_id, agent])?;
    }
    let mut st = conn.prepare_cached(
        "INSERT INTO diag_counts(file_id, category, name, version, count) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT DO UPDATE SET count=count+excluded.count",
    )?;
    for ((cat, name, version), n) in &rows.diag {
        st.execute(params![file_id, cat, name, version, n])?;
    }
    Ok(())
}

/// Deletes every row of a session (session-level reparse or removal).
pub fn delete_session(conn: &Connection, session_id: &str) -> CoreResult<()> {
    conn.execute(
        "DELETE FROM diag_counts WHERE file_id IN (SELECT id FROM files WHERE session_id=?1)",
        [session_id],
    )?;
    for table in [
        "msg_text",
        "messages",
        "tool_calls",
        "session_projects",
        "subagents",
        "persisted_outputs",
        "files",
    ] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE session_id=?1"),
            [session_id],
        )?;
    }
    conn.execute("DELETE FROM sessions WHERE id=?1", [session_id])?;
    Ok(())
}

/// Per-agent row of the `subagents` table (meta fields; times and tokens come from `messages`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentRow {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub parent_agent_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub workflow_run_id: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MetaLite {
    #[serde(default, deserialize_with = "lenient::string")]
    agent_type: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    parent_agent_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    tool_use_id: Option<String>,
}

impl AgentRow {
    /// Fills meta fields from an `agent-<id>.meta.json` body.
    pub fn with_meta(mut self, meta_json: Option<&str>) -> Self {
        if let Some(m) = meta_json.and_then(|s| serde_json::from_str::<MetaLite>(s).ok()) {
            self.agent_type = m.agent_type;
            self.parent_agent_id = m.parent_agent_id;
            self.tool_use_id = m.tool_use_id;
        }
        self
    }
}

/// Replaces the session's `subagents` rows (A9: refreshed on every session recompute).
pub fn write_subagents(conn: &Connection, session_id: &str, agents: &[AgentRow]) -> CoreResult<()> {
    conn.execute("DELETE FROM subagents WHERE session_id=?1", [session_id])?;
    let mut st = conn.prepare_cached(
        "INSERT INTO subagents(session_id, agent_id, agent_type, parent_agent_id, tool_use_id,
            workflow_run_id, started_ms, ended_ms, out_tok)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, min(ts_ms), max(ts_ms), coalesce(sum(out_tok), 0)
         FROM messages WHERE session_id=?1 AND agent_id=?2",
    )?;
    for a in agents {
        st.execute(params![
            session_id,
            a.agent_id,
            a.agent_type,
            a.parent_agent_id,
            a.tool_use_id,
            a.workflow_run_id,
        ])?;
    }
    Ok(())
}

/// A `sessions` row.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub project_id: String,
    pub cwd: Option<String>,
    pub title: String,
    pub title_source: TitleSource,
    pub first_prompt: Option<String>,
    pub created_ms: Option<f64>,
    pub last_active_ms: f64,
    pub duration_ms: f64,
    pub message_count: u32,
    pub tool_call_count: u32,
    pub subagent_count: u32,
    /// Main + subagents.
    pub tokens: TokenTotals,
    pub main_tokens: TokenTotals,
    pub git_branch: Option<String>,
    pub primary_model: Option<String>,
    pub models: Vec<String>,
    pub versions: Vec<String>,
    pub leaf_uuid: Option<String>,
    pub root_uuid: Option<String>,
    pub fork_origin_id: Option<String>,
    pub fork_point_uuid: Option<String>,
    pub is_empty: bool,
    /// Duplicate uuids within a single file (first occurrence wins), from assembly.
    pub dup_uuids: u32,
}

pub fn write_session(conn: &Connection, s: &SessionRow) -> CoreResult<()> {
    conn.prepare_cached(
        "INSERT OR REPLACE INTO sessions(id, project_id, cwd, title, title_source, first_prompt, created_ms,
            last_active_ms, duration_ms, message_count, tool_call_count, subagent_count,
            in_tok, out_tok, cr_tok, cc_tok, main_in_tok, main_out_tok, main_cr_tok, main_cc_tok,
            git_branch, primary_model, models_json, versions_json, leaf_uuid, root_uuid,
            fork_origin_id, fork_point_uuid, is_empty, dup_uuids)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
            ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
    )?
    .execute(params![
        s.id,
        s.project_id,
        s.cwd,
        s.title,
        title_source_to_db(s.title_source),
        s.first_prompt,
        s.created_ms.map(|v| v as i64),
        s.last_active_ms as i64,
        s.duration_ms as i64,
        s.message_count,
        s.tool_call_count,
        s.subagent_count,
        s.tokens.input as i64,
        s.tokens.output as i64,
        s.tokens.cache_read as i64,
        s.tokens.cache_creation as i64,
        s.main_tokens.input as i64,
        s.main_tokens.output as i64,
        s.main_tokens.cache_read as i64,
        s.main_tokens.cache_creation as i64,
        s.git_branch,
        s.primary_model,
        serde_json::to_string(&s.models)?,
        serde_json::to_string(&s.versions)?,
        s.leaf_uuid,
        s.root_uuid,
        s.fork_origin_id,
        s.fork_point_uuid,
        s.is_empty,
        s.dup_uuids,
    ])?;
    Ok(())
}

/// A `projects` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: String,
    pub path: String,
    pub display_name: String,
    pub missing: bool,
}

/// Inserts or updates a project; returns true when the row is new.
pub fn write_project(conn: &Connection, p: &ProjectRow) -> CoreResult<bool> {
    let existed = conn
        .prepare_cached("SELECT 1 FROM projects WHERE id=?1")?
        .query_row([&p.id], |_| Ok(()))
        .optional()?
        .is_some();
    conn.prepare_cached(
        "INSERT INTO projects(id, path, display_name, missing) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET path=excluded.path, display_name=excluded.display_name,
            missing=excluded.missing",
    )?
    .execute(params![p.id, p.path, p.display_name, p.missing])?;
    Ok(!existed)
}

/// Removes projects no session started in or edited; returns the number removed.
pub fn prune_projects(conn: &Connection) -> CoreResult<usize> {
    Ok(conn.execute(
        "DELETE FROM projects WHERE id NOT IN (SELECT project_id FROM sessions)
            AND id NOT IN (SELECT project_id FROM session_projects)",
        [],
    )?)
}

/// Distinct files the edit tools of a session (main and subagents) wrote; rejected or failed calls
/// wrote nothing.
pub fn edited_paths(conn: &Connection, session_id: &str) -> CoreResult<Vec<String>> {
    let mut st = conn.prepare_cached(
        "SELECT DISTINCT file_path FROM tool_calls
         WHERE session_id=?1 AND file_path IS NOT NULL AND is_error = 0",
    )?;
    let rows = st.query_map([session_id], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Replaces the Projects a session edited; returns true when the set changed.
pub fn write_session_projects(
    conn: &Connection,
    session_id: &str,
    ids: &[String],
) -> CoreResult<bool> {
    let mut st =
        conn.prepare_cached("SELECT project_id FROM session_projects WHERE session_id=?1")?;
    let before: HashSet<String> = st
        .query_map([session_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if before.len() == ids.len() && ids.iter().all(|id| before.contains(id)) {
        return Ok(false);
    }
    conn.execute(
        "DELETE FROM session_projects WHERE session_id=?1",
        [session_id],
    )?;
    let mut st = conn
        .prepare_cached("INSERT INTO session_projects(session_id, project_id) VALUES (?1, ?2)")?;
    for id in ids {
        st.execute(params![session_id, id])?;
    }
    Ok(true)
}

/// A2: recomputes `msg_text.on_main_line` for a session from its Main Line set. Subagent rows
/// stay on the main line (1).
pub fn write_on_main_line(
    conn: &Connection,
    session_id: &str,
    main_set: &HashSet<String>,
) -> CoreResult<()> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS temp_main(uuid TEXT PRIMARY KEY); DELETE FROM temp_main;",
    )?;
    let mut st = conn.prepare_cached("INSERT OR IGNORE INTO temp_main(uuid) VALUES (?1)")?;
    for u in main_set {
        st.execute([u])?;
    }
    conn.prepare_cached(
        "UPDATE msg_text SET on_main_line =
            (coalesce(agent_id, '') != '' OR node_uuid IN (SELECT uuid FROM temp_main))
         WHERE session_id=?1",
    )?
    .execute([session_id])?;
    conn.execute("DELETE FROM temp_main", [])?;
    Ok(())
}

/// Whether any `msg_text` row exists for the session (skip the A2 update otherwise).
pub fn has_text_rows(conn: &Connection, session_id: &str) -> CoreResult<bool> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM msg_text WHERE session_id=?1 LIMIT 1")?
        .query_row([session_id], |_| Ok(()))
        .optional()?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_bytes;

    #[test]
    fn extracts_cost_inputs_and_merges_fragments() {
        let lines = [
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[],"usage":{"input_tokens":1,"cache_creation_input_tokens":30,"cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20}}}}"#,
            r#"{"type":"assistant","uuid":"a2","timestamp":"2026-01-01T00:00:02Z","message":{"id":"m1","model":"x","content":[],"usage":{"output_tokens":7,"speed":"fast"}}}"#,
            r#"{"type":"assistant","uuid":"a3","timestamp":"2026-01-01T00:00:03Z","costUSD":0.25,"message":{"id":"m2","model":"x","content":[],"usage":{"input_tokens":1}}}"#,
        ];
        let buf = lines.join("\n") + "\n";
        let rows = extract_rows(&parse_bytes(buf.as_bytes(), 0).records);
        let [m1, m2] = &rows.messages[..] else {
            panic!("{:?}", rows.messages)
        };
        assert_eq!(m1.tokens, [1, 7, 0, 30, 20]);
        assert!(m1.fast);
        assert_eq!(m1.logged_cost, None);
        assert!(!m2.fast);
        assert_eq!(m2.logged_cost, Some(0.25));
    }

    #[test]
    fn extracts_messages_tools_and_persisted() {
        let lines = [
            r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"hello"}}"#,
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","model":"x","content":[{"type":"thinking","thinking":"t"}],"usage":{"input_tokens":3,"output_tokens":5}}}"#,
            r#"{"type":"assistant","uuid":"a2","timestamp":"2026-01-01T00:00:02Z","message":{"id":"m1","model":"x","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}},{"type":"tool_use","id":"t2","name":"Zed","input":{}}],"usage":{"input_tokens":3,"output_tokens":5}}}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2026-01-01T00:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"<persisted-output>\nOutput too large (42.4KB). Full output saved to: /x/y/tool-results/abc.txt\n\nPreview"}]},"toolUseResult":"Error"}"#,
            r#"{"type":"user","uuid":"u3","timestamp":"2026-01-01T00:00:04Z","isMeta":true,"message":{"role":"user","content":"<system-reminder>x"}}"#,
            r#"{"type":"hologram","uuid":"h1"}"#,
        ];
        let buf = lines.join("\n") + "\n";
        let chunk = parse_bytes(buf.as_bytes(), 0);
        let rows = extract_rows(&chunk.records);
        assert_eq!(rows.messages.len(), 2);
        assert_eq!(rows.messages[1].tokens, [3, 5, 0, 0, 0]);
        assert_eq!(rows.tool_calls.len(), 2);
        assert_eq!(rows.tool_errors, vec!["t1".to_owned()]);
        assert_eq!(rows.persisted[0].file_name, "abc.txt");
        assert_eq!(
            rows.diag.get(&(DIAG_TOOL, "Zed".into(), String::new())),
            Some(&1)
        );
        assert_eq!(
            rows.diag
                .get(&(DIAG_ENTRY, "hologram".into(), String::new())),
            Some(&1)
        );
    }
}
