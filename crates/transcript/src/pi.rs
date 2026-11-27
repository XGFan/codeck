//! Reading pi session files, for the conversation of a haa Worker that ran on pi.
//!
//! This is the only file in the repository that knows pi's on-disk format: where a session lives
//! (`<root>/sessions/<encoded cwd>/<time>_<session id>.jsonl`) and what its lines hold — one entry
//! per line (a `session` header, `message`s, `model_change`, …) linked into a tree by `id` /
//! `parentId`. [`read`] turns the messages into Claude Code entries on the same tree, so Branches
//! work as they do for Claude Code; pi's tools become Claude Code's (`bash` → `Bash`, `read` →
//! `Read`, `edit` → `Edit` / `MultiEdit`, `write` → `Write`, `grep` → `Grep`, `find` → `Glob`,
//! `ls` → `LS`), others keep their name. Entry types it does not know show as unrecognized entries.
//!
//! Callers: `codeck-core`'s display cache ([`find_session`], [`read`]), for a Session id that is
//! not in the index.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::foreign::{self, Emitter};
use crate::parse::ParsedChunk;

const TAG: &[u8] = b"codeck-pi-v1\0";

/// Entry types that carry nothing to show (settings, extension data, context pruning).
const QUIET_TYPES: &[&str] = &[
    "session",
    "model_change",
    "thinking_level_change",
    "custom",
    "context_edit",
    "label",
];

/// The file of session `id`: `<root>/sessions/*/<time>_<id>.jsonl`.
pub fn find_session(root: &Path, id: &str) -> Option<PathBuf> {
    if !foreign::is_plain_id(id) {
        return None;
    }
    let suffix = format!("_{id}.jsonl");
    foreign::subdirs(&root.join("sessions"))
        .iter()
        .find_map(|d| {
            foreign::files(d).into_iter().find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(&suffix))
            })
        })
}

/// Reads `path` (opened read-only) and adapts it; also returns the `\n` count consumed.
pub fn read(path: &Path, session_id: &str) -> io::Result<(ParsedChunk, u32)> {
    let buf = foreign::read_file(path)?;
    Ok(adapt(&buf, session_id))
}

/// Adapts the complete lines of a session file into Claude Code entries.
pub fn adapt(buf: &[u8], session_id: &str) -> (ParsedChunk, u32) {
    let mut out = Emitter::new(TAG, session_id);
    // pi entry id -> the nearest emitted entry at or above it (skipped entries pass their parent's on).
    let mut nearest: HashMap<String, Option<String>> = HashMap::new();
    let (lines, consumed_to, newlines) = foreign::lines(buf);
    for line in &lines {
        let v: Value = match serde_json::from_slice(line.bytes) {
            Ok(v) => v,
            Err(e) => {
                out.at(line, None);
                out.fail(e);
                continue;
            }
        };
        out.at(line, v.get("timestamp").and_then(Value::as_str));
        let parent = match v.get("parentId") {
            Some(Value::String(p)) => nearest.get(p).cloned().flatten(),
            Some(_) => None,
            // An entry without tree links follows the one before it.
            None => out.last(),
        };
        let kind = str_of(&v, "type").unwrap_or("");
        let emitted = match kind {
            "message" => v
                .get("message")
                .and_then(|m| message(&mut out, m, parent.clone())),
            k if QUIET_TYPES.contains(&k) => None,
            k => Some(out.push_under(foreign::unknown(&format!("pi:{k}")), parent.clone())),
        };
        if let Some(id) = str_of(&v, "id") {
            nearest.insert(id.to_owned(), emitted.or(parent));
        }
    }
    out.finish(consumed_to, newlines, buf.len() as u64)
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// One pi message; returns the last entry emitted for it.
fn message(out: &mut Emitter, m: &Value, parent: Option<String>) -> Option<String> {
    let role = str_of(m, "role").unwrap_or("");
    match role {
        "user" => Some(out.push_under(
            foreign::user(Value::Array(blocks(m.get("content")))),
            parent,
        )),
        "assistant" => {
            let content: Vec<Value> = m
                .get("content")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(assistant_block).collect())
                .unwrap_or_default();
            if content.is_empty() {
                let note = match (str_of(m, "stopReason"), str_of(m, "errorMessage")) {
                    (_, Some(e)) if !e.is_empty() => foreign::system("error", e),
                    (Some("aborted"), _) => foreign::system("info", "已中止"),
                    _ => return parent,
                };
                return Some(out.push_under(note, parent));
            }
            let id = out.fresh_id();
            let mut obj = foreign::assistant(&id, str_of(m, "model"), content);
            if let Some(u) = m.get("usage").filter(|u| u.is_object()) {
                let n = |k: &str| u.get(k).cloned().unwrap_or(Value::Null);
                obj["message"]["usage"] = json!({
                    "input_tokens": n("input"),
                    "output_tokens": n("output"),
                    "cache_read_input_tokens": n("cacheRead"),
                    "cache_creation_input_tokens": n("cacheWrite"),
                });
            }
            Some(out.push_under(obj, parent))
        }
        "toolResult" => {
            let call = str_of(m, "toolCallId").unwrap_or("");
            let content = blocks(m.get("content"));
            let text = content
                .iter()
                .filter_map(|b| str_of(b, "text"))
                .collect::<Vec<_>>()
                .join("\n");
            let failed = m.get("isError").and_then(Value::as_bool) == Some(true);
            let mut obj = foreign::tool_result(call, &text, failed, None);
            let images: Vec<Value> = content
                .into_iter()
                .filter(|b| str_of(b, "type") == Some("image"))
                .collect();
            if !images.is_empty() {
                // Text and pictures together, as Claude Code records a Read of an image.
                let mut parts = vec![json!({ "type": "text", "text": text })];
                parts.extend(images);
                obj["message"]["content"][0]["content"] = Value::Array(parts);
            }
            Some(out.push_under(obj, parent))
        }
        "system" => {
            let text = m
                .get("content")
                .map(|c| c.as_str().map_or_else(|| joined(c), str::to_owned));
            Some(out.push_under(foreign::system("info", &text.unwrap_or_default()), parent))
        }
        "bashExecution" => {
            // A command the person ran with `!`: shown like Claude Code's.
            let cmd = str_of(m, "command").unwrap_or("");
            let input = out.push_under(
                foreign::user(json!(format!("<bash-input>{cmd}</bash-input>"))),
                parent,
            );
            let output = str_of(m, "output").unwrap_or("");
            let shown = format!("<bash-stdout>{output}</bash-stdout><bash-stderr></bash-stderr>");
            Some(out.push_under(foreign::user(json!(shown)), Some(input)))
        }
        r => Some(out.push_under(foreign::unknown(&format!("pi:{r}")), parent)),
    }
}

/// pi content (a string or blocks) as Claude Code blocks: text and pictures.
fn blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(s)) => vec![json!({ "type": "text", "text": s })],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|b| match str_of(b, "type") {
                Some("text") => str_of(b, "text").map(|t| json!({ "type": "text", "text": t })),
                Some("image") => Some(foreign::image(
                    str_of(b, "mimeType").unwrap_or("image/png"),
                    str_of(b, "data").unwrap_or(""),
                )),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn joined(content: &Value) -> String {
    blocks(Some(content))
        .iter()
        .filter_map(|b| str_of(b, "text"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assistant_block(b: &Value) -> Option<Value> {
    match str_of(b, "type")? {
        "text" => str_of(b, "text")
            .filter(|t| !t.is_empty())
            .map(|t| json!({ "type": "text", "text": t })),
        "thinking" => str_of(b, "thinking")
            .filter(|t| !t.trim().is_empty())
            .map(|t| json!({ "type": "thinking", "thinking": t })),
        "toolCall" => {
            let empty = Value::Object(Map::new());
            let args = b
                .get("arguments")
                .filter(|a| a.is_object())
                .unwrap_or(&empty);
            let (name, input) = translate_tool(str_of(b, "name").unwrap_or(""), args);
            Some(
                json!({ "type": "tool_use", "id": str_of(b, "id").unwrap_or(""), "name": name, "input": input }),
            )
        }
        _ => None,
    }
}

/// pi's built-in tools as Claude Code's (name and input); others unchanged.
fn translate_tool(name: &str, args: &Value) -> (String, Value) {
    let get = |k: &str| args.get(k).cloned();
    let pick = |pairs: &[(&str, &str)]| {
        let mut m = Map::new();
        for (from, to) in pairs {
            if let Some(v) = get(from).filter(|v| !v.is_null()) {
                m.insert((*to).to_owned(), v);
            }
        }
        Value::Object(m)
    };
    match name {
        "bash" => (
            "Bash".into(),
            pick(&[("command", "command"), ("timeout", "timeout")]),
        ),
        "read" => (
            "Read".into(),
            pick(&[
                ("path", "file_path"),
                ("offset", "offset"),
                ("limit", "limit"),
            ]),
        ),
        "write" => (
            "Write".into(),
            pick(&[("path", "file_path"), ("content", "content")]),
        ),
        "grep" => (
            "Grep".into(),
            pick(&[
                ("pattern", "pattern"),
                ("path", "path"),
                ("glob", "glob"),
                ("ignoreCase", "-i"),
                ("context", "-C"),
                ("limit", "head_limit"),
            ]),
        ),
        "find" => (
            "Glob".into(),
            pick(&[("pattern", "pattern"), ("path", "path")]),
        ),
        "ls" => ("LS".into(), pick(&[("path", "path")])),
        "edit" => {
            let path = get("path").unwrap_or(Value::Null);
            let pair = |e: &Value| {
                json!({
                    "old_string": e.get("oldText").cloned().unwrap_or(Value::Null),
                    "new_string": e.get("newText").cloned().unwrap_or(Value::Null),
                })
            };
            match args.get("edits").and_then(Value::as_array) {
                Some(edits) if edits.len() == 1 => {
                    let mut e = pair(&edits[0]);
                    e["file_path"] = path;
                    ("Edit".into(), e)
                }
                Some(edits) => (
                    "MultiEdit".into(),
                    json!({ "file_path": path, "edits": edits.iter().map(pair).collect::<Vec<_>>() }),
                ),
                None => {
                    let mut e = pair(args);
                    e["file_path"] = path;
                    ("Edit".into(), e)
                }
            }
        }
        _ => (name.to_owned(), args.clone()),
    }
}
