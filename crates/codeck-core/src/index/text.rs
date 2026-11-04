//! Full-text document extraction and the FTS backlog (plan §4 "What gets indexed").
//!
//! Indexed: human/command prompt text and assistant `text` blocks (64 KB each), tool inputs
//! (16 KB; per-tool fields, else every JSON string leaf), AskUserQuestion answers and notes (as
//! the user's words, on the tool result entry). Never indexed: attachments, meta and system
//! entries, task notifications, compact summaries, thinking, images, other tool outputs.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use rusqlite::{Connection, params};
use serde_json::Value;

use crate::assemble::prompt;
use crate::error::{CoreError, CoreResult};
use crate::parse::{EntryRecord, parse_bytes};
use crate::raw::RawEntry;

/// Cap of a prompt or assistant text document.
pub const TEXT_CAP: usize = 64 * 1024;
/// Cap of a tool input document.
pub const TOOL_INPUT_CAP: usize = 16 * 1024;
/// Source bytes indexed per write transaction.
pub const BATCH_BYTES: u64 = 32 << 20;

/// `msg_text.role`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DocRole {
    User = 0,
    Assistant = 1,
    ToolInput = 2,
}

/// One row of `msg_text`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDoc {
    pub node_uuid: String,
    pub block_idx: u32,
    pub role: DocRole,
    pub ts_ms: Option<f64>,
    pub agent_id: Option<String>,
    /// Capped: 64 KB for prompts and assistant text, 16 KB for tool inputs.
    pub body: String,
    /// The tool_use id of a tool input document.
    pub tool_use_id: Option<String>,
}

/// Searchable documents of one entry (human/command prompts, assistant text, tool inputs).
pub fn extract_docs(rec: &EntryRecord, agent_id: Option<&str>) -> Vec<TextDoc> {
    let e = &rec.entry;
    let Some(uuid) = e.uuid.as_deref() else {
        return Vec::new();
    };
    let doc = |block_idx: usize, role, body: String, tool_use_id| TextDoc {
        node_uuid: uuid.to_owned(),
        block_idx: block_idx as u32,
        role,
        ts_ms: e.timestamp_ms(),
        agent_id: agent_id.map(str::to_owned),
        body,
        tool_use_id,
    };
    let mut out = Vec::new();
    match e.entry_type() {
        "user" => {
            let text = prompt::user_text(e);
            if prompt::is_human_prompt(e) && !text.trim().is_empty() {
                out.push(doc(0, DocRole::User, cap(text, TEXT_CAP), None));
            }
            if let Some((bi, body, tool_use_id)) = answer_doc(e) {
                out.push(doc(bi, DocRole::User, cap(body, TEXT_CAP), tool_use_id));
            }
        }
        "assistant" => {
            for (bi, b) in e.blocks().iter().enumerate() {
                match b.block_type() {
                    "text" => {
                        if let Some(t) = b.text_str().filter(|t| !t.trim().is_empty()) {
                            out.push(doc(bi, DocRole::Assistant, cap(t, TEXT_CAP), None));
                        }
                    }
                    "tool_use" => {
                        let name = b.name.as_deref().unwrap_or("");
                        let input = b
                            .input
                            .as_deref()
                            .and_then(|raw| serde_json::from_str::<Value>(raw.get()).ok());
                        let body = input.map(|v| tool_input_text(name, &v)).unwrap_or_default();
                        if !body.trim().is_empty() {
                            out.push(doc(
                                bi,
                                DocRole::ToolInput,
                                cap(body, TOOL_INPUT_CAP),
                                b.id.clone(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

/// AskUserQuestion answers and notes of a tool result entry: `(tool_result block index, text,
/// tool_use_id)`. Recognized by the `toolUseResult` shape (`questions` + `answers`), since the
/// result entry does not name its tool.
fn answer_doc(e: &RawEntry) -> Option<(usize, String, Option<String>)> {
    let raw = e.tool_use_result.as_deref()?;
    if !raw.get().starts_with('{') {
        return None;
    }
    let v: Value = serde_json::from_str(raw.get()).ok()?;
    let questions = v.get("questions")?;
    let answers = v.get("answers")?.as_object()?;
    let notes = v.get("annotations").and_then(Value::as_object);
    // In question order; answers keyed by anything else follow.
    let mut keys: Vec<&str> = questions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| q.get("question")?.as_str())
        .collect();
    let rest: Vec<&str> = answers
        .keys()
        .map(String::as_str)
        .filter(|k| !keys.contains(k))
        .collect();
    keys.extend(rest);
    let mut parts: Vec<&str> = Vec::new();
    for k in keys {
        parts.extend(answers.get(k).and_then(Value::as_str));
        parts.extend(notes.and_then(|n| n.get(k)?.get("notes")?.as_str()));
    }
    let body = parts.join("\n");
    if body.trim().is_empty() {
        return None;
    }
    let (bi, b) = e
        .blocks()
        .iter()
        .enumerate()
        .find(|(_, b)| b.block_type() == "tool_result")?;
    Some((bi, body, b.tool_use_id.clone()))
}

/// Indexed text of a tool input: the tool's key fields, else every string leaf.
fn tool_input_text(name: &str, input: &Value) -> String {
    let fields: &[&str] = match name {
        "Bash" => &["command", "description"],
        "Edit" => &["file_path", "old_string", "new_string"],
        "Write" => &["file_path", "content"],
        "Read" => &["file_path"],
        "Glob" => &["pattern", "path"],
        "Grep" => &["pattern", "path", "glob"],
        "Agent" | "Task" => &["description", "prompt"],
        _ => &[],
    };
    let mut parts: Vec<&str> = Vec::new();
    if fields.is_empty() {
        string_leaves(input, &mut parts);
    } else {
        parts.extend(fields.iter().filter_map(|f| input.get(*f)?.as_str()));
    }
    parts.join("\n")
}

fn string_leaves<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| string_leaves(x, out)),
        Value::Object(o) => o.values().for_each(|x| string_leaves(x, out)),
        _ => {}
    }
}

/// Truncates to at most `max` bytes on a char boundary.
fn cap(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// A `files` row with a text backlog.
struct Pending {
    id: i64,
    path: String,
    session_id: String,
    agent_id: Option<String>,
    is_main: bool,
    from: u64,
    to: u64,
}

/// Phase-2 coverage over every `files` row: a file is done when `text_offset >= parsed_offset`.
/// Totals stay stable while a backlog is interrupted and resumed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextProgress {
    pub files_done: u32,
    pub files_total: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// Indexes `[text_offset, parsed_offset)` of every file with a backlog. `main_set` returns the
/// Main Line uuid set of a session (A2) so `on_main_line` is set at insert time, or `None` when it
/// cannot be built: that session's files in the batch are then skipped and stay pending.
/// `progress` receives the coverage after every transaction.
///
/// Files are parsed in parallel and written in transactions of about `batch_bytes` source bytes
/// ([`BATCH_BYTES`] in the app). `cancel` is checked only after a transaction has committed, so
/// every call makes forward progress even when it is asked to yield right away; `Err(Cancelled)`
/// leaves the rest pending. Rows use `ON CONFLICT DO NOTHING` (A7), so copies of a Session index
/// each uuid once.
pub fn index_pending(
    conn: &mut Connection,
    main_set: &mut dyn FnMut(&str) -> Option<HashSet<String>>,
    progress: &dyn Fn(TextProgress),
    cancel: &AtomicBool,
    batch_bytes: u64,
) -> CoreResult<()> {
    let pending = {
        let mut st = conn.prepare(
            "SELECT id, path, session_id, agent_id, role, text_offset, parsed_offset FROM files
             WHERE text_offset < parsed_offset ORDER BY session_id, role, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Pending {
                id: r.get(0)?,
                path: r.get(1)?,
                session_id: r.get(2)?,
                agent_id: r.get(3)?,
                is_main: r.get::<_, i64>(4)? == super::schema::ROLE_MAIN,
                from: r.get::<_, i64>(5)? as u64,
                to: r.get::<_, i64>(6)? as u64,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut cov = coverage(conn)?;
    progress(cov);
    let mut rest = pending.as_slice();
    while !rest.is_empty() {
        let mut n = 0;
        let mut bytes = 0u64;
        while n < rest.len() && (n == 0 || bytes < batch_bytes) {
            bytes += rest[n].to - rest[n].from;
            n += 1;
        }
        let (batch, tail) = rest.split_at(n);
        rest = tail;
        let docs: Vec<Vec<TextDoc>> = batch.par_iter().map(file_docs).collect();
        let mut sets: HashMap<&str, Option<HashSet<String>>> = HashMap::new();
        for (p, d) in batch.iter().zip(&docs) {
            if p.is_main && !d.is_empty() && !sets.contains_key(p.session_id.as_str()) {
                sets.insert(&p.session_id, main_set(&p.session_id));
            }
        }
        let tx = conn.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO msg_text(file_id, session_id, agent_id, node_uuid, block_idx, role,
                    ts_ms, on_main_line, body, tool_use_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT DO NOTHING",
            )?;
            let mut upd = tx.prepare_cached("UPDATE files SET text_offset=?2 WHERE id=?1")?;
            for (p, file_docs) in batch.iter().zip(&docs) {
                let set = sets.get(p.session_id.as_str());
                if matches!(set, Some(None)) {
                    // Stays pending and out of the coverage.
                    n -= 1;
                    bytes -= p.to - p.from;
                    continue;
                }
                let set = set.and_then(Option::as_ref);
                for d in file_docs {
                    let on_main = !p.is_main || set.is_some_and(|s| s.contains(&d.node_uuid));
                    ins.execute(params![
                        p.id,
                        p.session_id,
                        d.agent_id,
                        d.node_uuid,
                        d.block_idx,
                        d.role as u8,
                        d.ts_ms.map(|t| t as i64),
                        on_main,
                        d.body,
                        d.tool_use_id,
                    ])?;
                }
                upd.execute(params![p.id, p.to as i64])?;
            }
        }
        tx.commit()?;
        cov.files_done += n as u32;
        cov.bytes_done += bytes;
        progress(cov);
        if !rest.is_empty() && cancel.load(Ordering::Relaxed) {
            return Err(CoreError::Cancelled);
        }
    }
    Ok(())
}

fn coverage(conn: &Connection) -> CoreResult<TextProgress> {
    Ok(conn.query_row(
        "SELECT count(*), coalesce(sum(text_offset >= parsed_offset), 0),
                coalesce(sum(parsed_offset), 0), coalesce(sum(min(text_offset, parsed_offset)), 0)
         FROM files",
        [],
        |r| {
            Ok(TextProgress {
                files_total: r.get(0)?,
                files_done: r.get(1)?,
                bytes_total: r.get::<_, i64>(2)? as u64,
                bytes_done: r.get::<_, i64>(3)? as u64,
            })
        },
    )?)
}

/// Reads `[from, to)` of a pending file and extracts its documents. An unreadable file yields
/// none (its offset still advances; a later rewrite triggers a session reparse).
fn file_docs(p: &Pending) -> Vec<TextDoc> {
    let read = || -> std::io::Result<Vec<u8>> {
        let mut f = File::open(&p.path)?;
        f.seek(SeekFrom::Start(p.from))?;
        let mut buf = Vec::with_capacity((p.to - p.from) as usize);
        f.take(p.to - p.from).read_to_end(&mut buf)?;
        Ok(buf)
    };
    let Ok(buf) = read() else {
        tracing::warn!(path = %p.path, "读取会话文件失败，跳过全文索引");
        return Vec::new();
    };
    let chunk = parse_bytes(&buf, p.from);
    chunk
        .records
        .iter()
        .flat_map(|r| extract_docs(r, p.agent_id.as_deref()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(line: &str) -> Vec<TextDoc> {
        let chunk = parse_bytes(format!("{line}\n").as_bytes(), 0);
        extract_docs(&chunk.records[0], Some("a1"))
    }

    #[test]
    fn extracts_prompts_text_and_tool_inputs() {
        let d = docs(r#"{"type":"user","uuid":"u1","message":{"content":"find the bug"}}"#);
        assert_eq!(d.len(), 1);
        assert_eq!(
            (d[0].role, d[0].body.as_str()),
            (DocRole::User, "find the bug")
        );
        assert_eq!(d[0].agent_id.as_deref(), Some("a1"));

        let d = docs(
            r#"{"type":"assistant","uuid":"a","message":{"id":"m","content":[{"type":"thinking","thinking":"secret"},{"type":"text","text":"Looking"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la","description":"List","timeout":5}},{"type":"tool_use","id":"t2","name":"Zed","input":{"a":{"b":["x","y"]},"n":1}}]}}"#,
        );
        assert_eq!(d.len(), 3);
        assert_eq!((d[0].block_idx, d[0].body.as_str()), (1, "Looking"));
        assert_eq!(d[1].body, "ls -la\nList");
        assert_eq!(d[1].tool_use_id.as_deref(), Some("t1"));
        assert_eq!(d[2].body, "x\ny");
    }

    #[test]
    fn skips_non_indexed_entries() {
        for line in [
            r#"{"type":"user","uuid":"u","isMeta":true,"message":{"content":"meta"}}"#,
            r#"{"type":"user","uuid":"u","isCompactSummary":true,"message":{"content":"summary"}}"#,
            r#"{"type":"user","uuid":"u","message":{"content":"<task-notification><summary>s</summary></task-notification>"}}"#,
            r#"{"type":"user","uuid":"u","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"out"}]}}"#,
            r#"{"type":"attachment","uuid":"u","attachment":{"type":"x","content":"hook"}}"#,
            r#"{"type":"system","uuid":"u","content":"sys"}"#,
        ] {
            assert!(docs(line).is_empty(), "{line}");
        }
    }

    #[test]
    fn indexes_ask_answers_as_user_words() {
        let d = docs(
            r#"{"type":"user","uuid":"r","message":{"content":[{"type":"tool_result","tool_use_id":"t9","content":"User has answered"}]},"toolUseResult":{"questions":[{"question":"Q1"},{"question":"Q2"}],"answers":{"Q2":"自定义的命名","Q1":"429 + Retry-After"},"annotations":{"Q1":{"notes":"记得加 Retry-After 头","preview":"x"}}}}"#,
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].role, DocRole::User);
        assert_eq!(d[0].tool_use_id.as_deref(), Some("t9"));
        assert_eq!(
            d[0].body,
            "429 + Retry-After\n记得加 Retry-After 头\n自定义的命名"
        );

        // Other tools' structured results stay out.
        let d = docs(
            r#"{"type":"user","uuid":"r","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]},"toolUseResult":{"stdout":"answers","stderr":""}}"#,
        );
        assert!(d.is_empty());
    }

    #[test]
    fn caps_on_char_boundary() {
        let s = cap("é".repeat(40_000), TEXT_CAP);
        assert!(s.len() <= TEXT_CAP && s.len() > TEXT_CAP - 2);
    }
}
