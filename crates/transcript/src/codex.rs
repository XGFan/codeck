//! Reading codex rollout files, for the conversation of a haa Worker that ran on codex.
//!
//! This is the only file in the repository that knows codex's on-disk format: where a thread's
//! rollout lives (`<root>/sessions/YYYY/MM/DD/rollout-<time>-<thread id>.jsonl`, or
//! `<root>/archived_sessions/`) and what its lines hold. A rollout records each turn twice: as the
//! model saw it (`response_item`: developer instructions, the `exec` script through which the
//! model calls its tools) and as codex's app shows it (`event_msg` / `item_completed`: messages,
//! commands, file changes, MCP calls). Only the app's view is adapted: [`read`] turns it into a
//! straight `parentUuid`-chained sequence of Claude Code entries (a command is a `Bash` call, a
//! file change a `Write` / `Edit` call with its diff), so assembly, the tool renderers and ⌘F treat
//! it like any transcript. Item types it does not know show as unrecognized entries.
//!
//! Callers: `codeck-core`'s display cache ([`find_rollout`], [`read`]), for a Session id that is
//! not in the index.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::foreign::{self, Emitter};
use crate::parse::ParsedChunk;

const TAG: &[u8] = b"codeck-codex-v1\0";

/// The rollout of `thread_id`: `<root>/sessions/<Y>/<M>/<D>/rollout-*-<thread_id>.jsonl` (newest days
/// first), else `<root>/archived_sessions/`; codex compresses old ones to `….jsonl.zst`. A thread codex
/// continued in another file (`…-<thread id>_<part id>.jsonl`) is read up to its first file only.
pub fn find_rollout(root: &Path, thread_id: &str) -> Option<PathBuf> {
    if !foreign::is_plain_id(thread_id) {
        return None;
    }
    let (plain, packed) = (
        format!("-{thread_id}.jsonl"),
        format!("-{thread_id}.jsonl.zst"),
    );
    let hit = |p: &PathBuf| {
        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n.starts_with("rollout-") && (n.ends_with(&plain) || n.ends_with(&packed))
        })
    };
    for y in foreign::subdirs(&root.join("sessions")).iter().rev() {
        for m in foreign::subdirs(y).iter().rev() {
            for d in foreign::subdirs(m).iter().rev() {
                if let Some(p) = foreign::files(d).into_iter().find(hit) {
                    return Some(p);
                }
            }
        }
    }
    foreign::files(&root.join("archived_sessions"))
        .into_iter()
        .find(hit)
}

/// Reads `path` (opened read-only; a `.zst` one decompressed) and adapts it; also returns the `\n`
/// count consumed.
pub fn read(path: &Path, thread_id: &str) -> io::Result<(ParsedChunk, u32)> {
    let mut buf = foreign::read_file(path)?;
    if path.extension().is_some_and(|e| e == "zst") {
        buf = unzstd(&buf)?;
    }
    Ok(adapt(&buf, thread_id))
}

/// Decompresses every zstd frame of `packed`.
fn unzstd(packed: &[u8]) -> io::Result<Vec<u8>> {
    let mut src = packed;
    let mut out = Vec::new();
    while !src.is_empty() {
        let mut dec = ruzstd::decoding::StreamingDecoder::new(&mut src)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        dec.read_to_end(&mut out)?;
    }
    Ok(out)
}

/// Adapts the complete lines of a rollout into Claude Code entries.
pub fn adapt(buf: &[u8], thread_id: &str) -> (ParsedChunk, u32) {
    let mut out = Emitter::new(TAG, thread_id);
    let mut model: Option<String> = None;
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
        let Some(p) = v.get("payload") else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("turn_context") => {
                if let Some(m) = str_of(p, "model") {
                    model = Some(m.to_owned());
                }
            }
            Some("event_msg") => match str_of(p, "type") {
                Some("item_completed") => {
                    if let Some(item) = p.get("item") {
                        adapt_item(&mut out, item, model.as_deref());
                    }
                }
                Some("turn_aborted") => {
                    out.push(foreign::system("info", "本轮已中止"));
                }
                Some("error") => {
                    out.push(foreign::system(
                        "error",
                        str_of(p, "message").unwrap_or("出错"),
                    ));
                }
                _ => {}
            },
            _ => {}
        }
    }
    out.finish(consumed_to, newlines, buf.len() as u64)
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn adapt_item(out: &mut Emitter, item: &Value, model: Option<&str>) {
    let id = str_of(item, "id")
        .filter(|s| !s.is_empty())
        .map_or_else(|| out.fresh_id(), str::to_owned);
    let kind = str_of(item, "type").unwrap_or("");
    match kind {
        "UserMessage" => {
            let blocks = user_blocks(item.get("content"));
            if !blocks.is_empty() {
                out.push(foreign::user(Value::Array(blocks)));
            }
        }
        "AgentMessage" => {
            let text = texts(item.get("content"), "\n\n");
            if !text.is_empty() {
                out.push(foreign::assistant(
                    &id,
                    model,
                    vec![json!({ "type": "text", "text": text })],
                ));
            }
        }
        "Reasoning" => {
            let mut parts = strings(item.get("summary_text"));
            parts.extend(strings(item.get("raw_content")));
            let text = parts.join("\n\n");
            if !text.trim().is_empty() {
                out.push(foreign::assistant(
                    &id,
                    model,
                    vec![json!({ "type": "thinking", "thinking": text })],
                ));
            }
        }
        "CommandExecution" => {
            let command = command_line(item.get("command"));
            let mut text = str_of(item, "aggregated_output").unwrap_or("").to_owned();
            let code = item.get("exit_code").and_then(Value::as_i64);
            let failed = str_of(item, "status").is_some_and(|s| s != "completed")
                || code.is_some_and(|c| c != 0);
            if let Some(c) = code.filter(|c| *c != 0) {
                text = format!("Exit code {c}\n{text}");
            }
            tool(
                out,
                &id,
                "Bash",
                json!({ "command": command }),
                &text,
                failed,
                None,
                model,
            );
        }
        "FileChange" => file_change(out, &id, item, model),
        "McpToolCall" => {
            let name = format!(
                "mcp__{}__{}",
                str_of(item, "server").unwrap_or(""),
                str_of(item, "tool").unwrap_or("")
            );
            let input = item
                .get("arguments")
                .filter(|a| a.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = item.get("result");
            let mut text = texts(result.and_then(|r| r.get("content")), "\n");
            if let Some(e) = item.get("error").filter(|e| !e.is_null()) {
                text = str_of(e, "message").map_or_else(|| e.to_string(), str::to_owned);
            }
            let failed = str_of(item, "status") == Some("failed")
                || result
                    .and_then(|r| r.get("isError"))
                    .and_then(Value::as_bool)
                    == Some(true);
            tool(out, &id, &name, input, &text, failed, None, model);
        }
        "ContextCompaction" => {
            out.push(foreign::system("info", "上下文已压缩"));
        }
        "Plan" => {
            if let Some(text) = str_of(item, "text").filter(|t| !t.is_empty()) {
                out.push(foreign::assistant(
                    &id,
                    model,
                    vec![json!({ "type": "text", "text": text })],
                ));
            }
        }
        "WebSearch" | "Extension"
            if kind == "WebSearch" || str_of(item, "kind") == Some("web.search") =>
        {
            let query = str_of(item, "query").map(str::to_owned).unwrap_or_else(|| {
                strings(item.get("action").and_then(|a| a.get("queries"))).join(" · ")
            });
            tool(
                out,
                &id,
                "WebSearch",
                json!({ "query": query }),
                "",
                false,
                None,
                model,
            );
        }
        "Extension" | "ImageView" | "DynamicToolCall" => {
            let name = str_of(item, "tool")
                .or_else(|| str_of(item, "kind"))
                .unwrap_or(kind)
                .to_owned();
            let mut input = item.as_object().cloned().unwrap_or_default();
            for k in [
                "type",
                "id",
                "kind",
                "status",
                "result",
                "output",
                "contentItems",
                "success",
            ] {
                input.remove(k);
            }
            if let Some(args) = input.remove("arguments").filter(|a| a.is_object()) {
                input = args.as_object().cloned().unwrap_or_default();
            }
            let text = ["contentItems", "output", "result"]
                .iter()
                .find_map(|k| item.get(*k).filter(|v| !v.is_null()))
                .map(|v| {
                    let t = texts(Some(v), "\n");
                    if t.is_empty() {
                        v.as_str().map_or_else(|| v.to_string(), str::to_owned)
                    } else {
                        t
                    }
                })
                .unwrap_or_default();
            let failed = str_of(item, "status") == Some("failed")
                || item.get("success") == Some(&Value::Bool(false));
            tool(
                out,
                &id,
                &name,
                Value::Object(input),
                &text,
                failed,
                None,
                model,
            );
        }
        _ => {
            out.push(foreign::unknown(&format!("codex:{kind}")));
        }
    }
}

/// One codex `FileChange` item: a `Write` / `Edit` / `Delete` call per file.
fn file_change(out: &mut Emitter, id: &str, item: &Value, model: Option<&str>) {
    let declined = str_of(item, "status").is_some_and(|s| s != "completed");
    let note = [str_of(item, "stderr"), str_of(item, "stdout")]
        .into_iter()
        .flatten()
        .find(|t| !t.is_empty())
        .unwrap_or(if declined { "未应用" } else { "" })
        .to_owned();
    let Some(changes) = item.get("changes").and_then(Value::as_object) else {
        return;
    };
    for (k, (path, ch)) in changes.iter().enumerate() {
        let tid = format!("{id}#{k}");
        let text = |ok: String| if declined { note.clone() } else { ok };
        match str_of(ch, "type") {
            Some("add") => {
                let input =
                    json!({ "file_path": path, "content": str_of(ch, "content").unwrap_or("") });
                let t = text(format!("File created successfully at: {path}"));
                tool(out, &tid, "Write", input, &t, declined, None, model);
            }
            Some("delete") => {
                let t = text(format!("Deleted {path}"));
                tool(
                    out,
                    &tid,
                    "Delete",
                    json!({ "file_path": path }),
                    &t,
                    declined,
                    None,
                    model,
                );
            }
            _ => {
                let diff = str_of(ch, "unified_diff").unwrap_or("");
                let mut input = json!({ "file_path": path });
                if let Some(to) = str_of(ch, "move_path") {
                    input["move_to"] = Value::String(to.to_owned());
                }
                let hunks = foreign::structured_patch(diff);
                let t = text(format!("Updated {path}"));
                if hunks.is_empty() {
                    input["diff"] = Value::String(diff.to_owned());
                    tool(out, &tid, "apply_patch", input, &t, declined, None, model);
                } else {
                    let extra = json!({ "structuredPatch": hunks });
                    tool(out, &tid, "Edit", input, &t, declined, Some(extra), model);
                }
            }
        }
    }
}

/// A tool call and its result, as an `assistant` entry and a `user` entry.
#[allow(clippy::too_many_arguments)]
fn tool(
    out: &mut Emitter,
    id: &str,
    name: &str,
    input: Value,
    text: &str,
    failed: bool,
    extra: Option<Value>,
    model: Option<&str>,
) {
    let tool_use = json!({ "type": "tool_use", "id": id, "name": name, "input": input });
    out.push(foreign::assistant(id, model, vec![tool_use]));
    out.push(foreign::tool_result(id, text, failed, extra));
}

/// A `UserMessage`'s content as Claude Code blocks: text, and pictures sent as data URLs.
fn user_blocks(content: Option<&Value>) -> Vec<Value> {
    let Some(items) = content.and_then(Value::as_array) else {
        return content
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| vec![json!({ "type": "text", "text": s })])
            .unwrap_or_default();
    };
    items
        .iter()
        .filter_map(|b| match str_of(b, "type") {
            Some("text") => str_of(b, "text").map(|t| json!({ "type": "text", "text": t })),
            Some("image") => str_of(b, "url")
                .or_else(|| str_of(b, "image_url"))
                .and_then(foreign::image_from_data_url),
            Some("localImage") | Some("local_image") => {
                str_of(b, "path").map(|p| json!({ "type": "text", "text": format!("[图片] {p}") }))
            }
            _ => None,
        })
        .collect()
}

/// Joined text of content items (`{ type: text | Text | output_text | input_text, text }`).
fn texts(content: Option<&Value>, sep: &str) -> String {
    let Some(items) = content.and_then(Value::as_array) else {
        return String::new();
    };
    items
        .iter()
        .filter(|b| {
            matches!(
                str_of(b, "type"),
                Some("text" | "Text" | "output_text" | "input_text")
            )
        })
        .filter_map(|b| str_of(b, "text"))
        .collect::<Vec<_>>()
        .join(sep)
}

/// Non-empty strings of an array (of strings or `{ text }` objects).
fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().or_else(|| str_of(x, "text")))
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The command as typed: the script of `<shell> -lc <script>`, else the words joined.
fn command_line(command: Option<&Value>) -> String {
    match command {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(words)) => {
            let w: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
            if w.len() == 3 && matches!(w[1], "-lc" | "-c") {
                w[2].to_owned()
            } else {
                w.iter()
                    .map(|s| {
                        if s.contains(char::is_whitespace) || s.is_empty() {
                            format!("'{}'", s.replace('\'', "'\\''"))
                        } else {
                            (*s).to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        }
        _ => String::new(),
    }
}
