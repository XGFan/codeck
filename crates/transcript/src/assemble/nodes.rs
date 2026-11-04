//! Normalization into `model::Node`: merge assistant fragments, pair tool calls with results,
//! classify satellites and hidden entries, apply payload caps (§3.4 steps 5–6).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::{LoadedFile, SessionSkeleton, content, prompt, tree};
use crate::known;
use crate::model::{
    AgentId, AssistantBlock, ImageRef, Node, NodeBody, NodeId, PersistedRef, TokenTotals, ToolCall,
    ToolResult,
};
use crate::raw::{RawBlock, RawEntry, decode_str};

/// tool_result text preview cap (bytes / lines).
pub const RESULT_BYTES: usize = 2 * 1024;
pub const RESULT_LINES: usize = 40;
/// Cap per string value inside a tool input.
pub const INPUT_STRING_BYTES: usize = 4 * 1024;
/// Attachment / system / unknown preview cap.
pub const PREVIEW_BYTES: usize = 1024;
/// `ToolResult.extra_json` cap (whole fields dropped, largest first).
pub const EXTRA_BYTES: usize = 8 * 1024;

pub struct RenderCtx<'a> {
    pub skeleton: &'a SessionSkeleton,
    /// The files the skeleton was built from.
    pub files: &'a [LoadedFile],
    pub include_hidden: bool,
    pub subagent_by_tool: &'a HashMap<String, AgentId>,
    pub workflow_by_tool: &'a HashMap<String, String>,
    pub notification_by_tool: &'a HashMap<String, NodeId>,
    /// Index fallback of §3.4 step 8: display entries up to this node are fork-inherited.
    pub fork_point: Option<usize>,
}

pub struct Rendered {
    pub nodes: Vec<Node>,
    /// Hidden nodes in the display set (emitted only with `include_hidden`).
    pub hidden_count: u32,
}

/// Renders the display entries (see [`tree::display_entries`]) into nodes.
pub fn render(ctx: &RenderCtx<'_>, display: &[usize]) -> Rendered {
    let s = ctx.skeleton;
    let mut shown = vec![false; s.nodes.len()];
    for &i in display {
        shown[i] = true;
    }
    let fork_pos = ctx
        .fork_point
        .and_then(|f| display.iter().position(|&x| x == f));
    let mut emitted_messages: HashSet<&str> = HashSet::new();
    let mut nodes = Vec::with_capacity(display.len());
    let mut hidden_count = 0u32;

    for (pos, &i) in display.iter().enumerate() {
        let n = &s.nodes[i];
        let e = s.entry(ctx.files, i);
        let mut id_idx = i;
        let mut hidden = false;
        let body = match n.entry_type.as_str() {
            "assistant" => {
                let frags: &[usize] = match n.message_id.as_deref() {
                    Some(m) => {
                        if !emitted_messages.insert(m) {
                            continue;
                        }
                        &s.fragments[m]
                    }
                    None => std::slice::from_ref(&display[pos]),
                };
                id_idx = frags[0];
                assistant_body(ctx, frags, &shown)
            }
            "user" if n.is_compact_summary => NodeBody::CompactSummary {
                text: prompt::user_text(e),
            },
            "user" if n.is_tool_result => {
                if !n.tool_results.is_empty()
                    && n.tool_results
                        .iter()
                        .all(|id| s.tool_use_at.contains_key(id))
                {
                    continue; // absorbed into its ToolCall
                }
                hidden = true;
                let text = n
                    .tool_results
                    .iter()
                    .find_map(|id| {
                        e.blocks()
                            .iter()
                            .find(|b| b.tool_use_id.as_ref() == Some(id))
                    })
                    .map(|b| result_content(b.content.as_deref()).0)
                    .unwrap_or_default();
                NodeBody::System {
                    subtype: "tool_result".to_owned(),
                    level: None,
                    text: cap_text(&text, PREVIEW_BYTES, RESULT_LINES).0,
                }
            }
            "user" => {
                let text = prompt::user_text(e);
                let origin = prompt::classify(e, &text);
                hidden = n.is_meta;
                NodeBody::UserPrompt {
                    images: prompt_images(e, &n.uuid),
                    text,
                    origin,
                }
            }
            "system" if n.is_compact_boundary() => {
                let meta = e.compact_metadata.as_ref();
                NodeBody::CompactBoundary {
                    trigger: meta.and_then(|m| m.trigger.clone()),
                    pre_tokens: meta.and_then(|m| m.pre_tokens),
                }
            }
            "system" => {
                let subtype = n.subtype.clone().unwrap_or_default();
                hidden = known::is_hidden_system_subtype(&subtype);
                let mut text = system_text(ctx, i, e);
                if text.trim().is_empty() {
                    text = empty_system_text(&subtype);
                }
                NodeBody::System {
                    subtype,
                    level: e.level.clone(),
                    text,
                }
            }
            "attachment" => {
                hidden = true;
                let (attachment_type, text) = attachment_preview(e.attachment.as_deref());
                NodeBody::Attachment {
                    attachment_type,
                    text,
                }
            }
            other => {
                let (path, offset) = s.location(ctx.files, i);
                NodeBody::Unknown {
                    entry_type: other.to_owned(),
                    raw_json: raw_line(path, offset)
                        .map(|l| cap_json(&l, PREVIEW_BYTES).0)
                        .unwrap_or_else(|| "{}".to_owned()),
                }
            }
        };
        if hidden {
            hidden_count += 1;
            if !ctx.include_hidden {
                continue;
            }
        }
        let head = &s.nodes[id_idx];
        nodes.push(Node {
            id: head.uuid.clone(),
            timestamp_ms: head.timestamp_ms,
            hidden,
            inherited: head.forked || fork_pos.is_some_and(|fp| pos <= fp),
            body,
        });
    }
    Rendered {
        nodes,
        hidden_count,
    }
}

fn assistant_body(ctx: &RenderCtx<'_>, frags: &[usize], shown: &[bool]) -> NodeBody {
    let s = ctx.skeleton;
    let node_id = &s.nodes[frags[0]].uuid;
    let mut blocks = Vec::new();
    let mut model = None;
    let mut usage: Option<TokenTotals> = None;
    let mut is_api_error = false;
    let mut message_id = None;
    for &f in frags {
        let n = &s.nodes[f];
        let e = s.entry(ctx.files, f);
        message_id = message_id.or_else(|| n.message_id.clone());
        model = model.or_else(|| n.model.clone());
        is_api_error |= n.is_api_error;
        if let Some(u) = &n.usage {
            let t = usage.get_or_insert_with(TokenTotals::default);
            t.input = t.input.max(u.input);
            t.output = t.output.max(u.output);
            t.cache_read = t.cache_read.max(u.cache_read);
            t.cache_creation = t.cache_creation.max(u.cache_creation);
        }
        for (bi, b) in e.blocks().iter().enumerate() {
            match b.block_type() {
                "text" => blocks.push(AssistantBlock::Text {
                    text: b.text_str().unwrap_or_default(),
                }),
                "thinking" => {
                    if let Some(t) = b.thinking_str().filter(|t| !t.trim().is_empty()) {
                        blocks.push(AssistantBlock::Thinking { text: t });
                    }
                }
                "redacted_thinking" => {}
                "tool_use" => {
                    blocks.push(AssistantBlock::ToolCall(tool_call(ctx, b, shown, node_id)))
                }
                other => {
                    let (path, offset) = s.location(ctx.files, f);
                    blocks.push(AssistantBlock::Unknown {
                        block_type: other.to_owned(),
                        raw_json: raw_block(path, offset, bi)
                            .map(|raw| cap_json(&raw, PREVIEW_BYTES).0)
                            .unwrap_or_else(|| "{}".to_owned()),
                    });
                }
            }
        }
    }
    NodeBody::Assistant {
        message_id,
        model,
        blocks,
        usage,
        is_api_error,
    }
}

fn tool_call(ctx: &RenderCtx<'_>, b: &RawBlock, shown: &[bool], node_id: &str) -> ToolCall {
    let s = ctx.skeleton;
    let id = b.id.clone().unwrap_or_default();
    let name = b.name.clone().unwrap_or_default();
    let (input_json, input_truncated) = match b.input.as_deref() {
        Some(raw) => cap_json(raw.get(), INPUT_STRING_BYTES),
        None => ("{}".to_owned(), false),
    };
    let result = tree::chosen_result(s, &id, shown).and_then(|r| {
        let e = s.entry(ctx.files, r);
        let block = e.blocks().iter().find(|rb| {
            rb.block_type() == "tool_result" && rb.tool_use_id.as_deref() == Some(&id)
        })?;
        Some(tool_result(e, block, &name, node_id, &id))
    });
    ToolCall {
        subagent_id: ctx.subagent_by_tool.get(&id).cloned(),
        workflow_run_id: ctx.workflow_by_tool.get(&id).cloned(),
        notification_node_id: ctx.notification_by_tool.get(&id).cloned(),
        tool_use_id: id,
        name,
        input_json,
        input_truncated,
        result,
    }
}

/// Builds a capped `ToolResult` from the tool_result `block` of entry `e`.
pub fn tool_result(
    e: &RawEntry,
    block: &RawBlock,
    tool_name: &str,
    node_id: &str,
    tool_use_id: &str,
) -> ToolResult {
    let (full, images) = result_content(block.content.as_deref());
    let (text, truncated) = cap_text(&full, RESULT_BYTES, RESULT_LINES);
    ToolResult {
        is_error: block.is_error == Some(true),
        timestamp_ms: e.timestamp_ms(),
        images: images
            .into_iter()
            .enumerate()
            .map(|(k, (media_type, bytes))| ImageRef {
                node_id: node_id.to_owned(),
                ordinal: k as u32,
                media_type,
                bytes,
                tool_use_id: Some(tool_use_id.to_owned()),
            })
            .collect(),
        total_bytes: full.len() as f64,
        truncated,
        persisted: persisted_ref(e, &full),
        extra_json: extra_json(e.tool_use_result.as_deref(), tool_name),
        text,
    }
}

/// §3.4 step 12: a `<persisted-output>` result or `toolUseResult.persistedOutputPath` gives a
/// basename-only reference (the absolute path is never used).
pub fn persisted_ref(e: &RawEntry, text: &str) -> Option<PersistedRef> {
    let has_field = e.tool_use_result.as_deref().is_some_and(|r| {
        memchr::memmem::find(r.get().as_bytes(), b"\"persistedOutputPath\"").is_some()
    });
    let fields = has_field.then(|| e.tool_use_result_fields()).flatten();
    let from_field = fields
        .as_ref()
        .and_then(|f| f.persisted_output_path.clone());
    let path = from_field.or_else(|| {
        if !text.starts_with("<persisted-output>") {
            return None;
        }
        let marker = "saved to: ";
        let start = text.find(marker)? + marker.len();
        let line = text[start..].lines().next()?.trim();
        Some(line.to_owned())
    })?;
    Some(PersistedRef {
        file_name: content::persisted_basename(&path)?,
        size_bytes: fields.and_then(|f| f.persisted_output_size).unwrap_or(0.0),
    })
}

#[derive(Deserialize)]
struct ContentItem<'a> {
    #[serde(rename = "type", default, borrow)]
    item_type: Option<&'a RawValue>,
    #[serde(default, borrow)]
    text: Option<&'a RawValue>,
    #[serde(default, borrow)]
    source: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct ImageSource<'a> {
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default, borrow)]
    data: Option<Cow<'a, str>>,
}

/// Full text (text items joined by `\n`) and images `(media_type, bytes)` of a tool_result
/// `content` (a string or an array of blocks).
pub fn result_content(raw: Option<&RawValue>) -> (String, Vec<(String, f64)>) {
    let Some(raw) = raw else {
        return (String::new(), Vec::new());
    };
    let s = raw.get();
    if s.starts_with('"') {
        return (decode_str(Some(raw)).unwrap_or_default(), Vec::new());
    }
    let mut text = String::new();
    let mut images = Vec::new();
    if let Ok(items) = serde_json::from_str::<Vec<ContentItem<'_>>>(s) {
        for it in items {
            match it.item_type.and_then(|t| decode_str(Some(t))).as_deref() {
                Some("text") => {
                    if let Some(t) = decode_str(it.text) {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&t);
                    }
                }
                Some("image") => images.push(image_info(it.source)),
                _ => {}
            }
        }
    }
    (text, images)
}

/// `(media_type, decoded byte estimate)` of an image `source`.
pub fn image_info(source: Option<&RawValue>) -> (String, f64) {
    let src = source.and_then(|s| serde_json::from_str::<ImageSource<'_>>(s.get()).ok());
    let media_type = src
        .as_ref()
        .and_then(|s| s.media_type.clone())
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    let bytes = src
        .and_then(|s| s.data)
        .map(|d| (d.trim_end_matches('=').len() * 3 / 4) as f64)
        .unwrap_or(0.0);
    (media_type, bytes)
}

fn prompt_images(e: &RawEntry, node_id: &str) -> Vec<ImageRef> {
    e.blocks()
        .iter()
        .filter(|b| b.block_type() == "image")
        .enumerate()
        .map(|(k, b)| {
            let (media_type, bytes) = image_info(b.source.as_deref());
            ImageRef {
                node_id: node_id.to_owned(),
                ordinal: k as u32,
                media_type,
                bytes,
                tool_use_id: None,
            }
        })
        .collect()
}

/// toolUseResult fields kept per tool (plan §3.1).
fn extra_fields(tool: &str) -> &'static [&'static str] {
    match tool {
        "Bash" => &[
            "stdout",
            "stderr",
            "interrupted",
            "returnCodeInterpretation",
            "backgroundTaskId",
        ],
        "AskUserQuestion" => &["questions", "answers", "annotations"],
        "Edit" => &["structuredPatch"],
        t if t.starts_with("Task") && t != "Task" => &["task", "statusChange"],
        _ => &[],
    }
}

/// A11: the tool's subset of an object `toolUseResult`; whole fields are dropped (largest
/// first) until it fits [`EXTRA_BYTES`]. Strings are never sliced.
pub fn extra_json(raw: Option<&RawValue>, tool: &str) -> Option<String> {
    let wanted = extra_fields(tool);
    let raw = raw?;
    if wanted.is_empty() || !raw.get().starts_with('{') {
        return None;
    }
    let map: HashMap<String, &RawValue> = serde_json::from_str(raw.get()).ok()?;
    let mut fields: Vec<(&str, &RawValue)> = wanted
        .iter()
        .filter_map(|k| map.get(*k).map(|v| (*k, *v)))
        .filter(|(_, v)| v.get() != "null")
        .collect();
    let size = |f: &[(&str, &RawValue)]| -> usize {
        2 + f
            .iter()
            .map(|(k, v)| k.len() + v.get().len() + 4)
            .sum::<usize>()
    };
    while !fields.is_empty() && size(&fields) > EXTRA_BYTES {
        let largest = fields
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, v))| v.get().len())
            .map(|(i, _)| i)
            .unwrap_or(0);
        fields.remove(largest);
    }
    if fields.is_empty() {
        return None;
    }
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("\"{k}\":{}", v.get()))
        .collect();
    Some(format!("{{{}}}", body.join(",")))
}

/// Caps `s` at `max_bytes` (char boundary) and `max_lines` lines; returns `(text, truncated)`.
pub fn cap_text(s: &str, max_bytes: usize, max_lines: usize) -> (String, bool) {
    let mut cut = s.len();
    if let Some((i, _)) = s.match_indices('\n').nth(max_lines.saturating_sub(1)) {
        cut = i;
    }
    if cut > max_bytes {
        cut = max_bytes;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
    }
    if cut >= s.len() {
        (s.to_owned(), false)
    } else {
        (s[..cut].to_owned(), true)
    }
}

/// Re-serializes JSON with every string value capped at `max_string` bytes (always valid JSON).
/// Returns `(json, truncated)`; inputs too short to need a cap are returned verbatim.
pub fn cap_json(raw: &str, max_string: usize) -> (String, bool) {
    if raw.len() <= max_string {
        return (raw.to_owned(), false);
    }
    let Ok(mut v) = serde_json::from_str::<Value>(raw) else {
        let (t, _) = cap_text(raw, max_string, usize::MAX);
        return (Value::String(t).to_string(), true);
    };
    let truncated = cap_strings(&mut v, max_string);
    if !truncated {
        return (raw.to_owned(), false);
    }
    (v.to_string(), true)
}

fn cap_strings(v: &mut Value, max: usize) -> bool {
    match v {
        Value::String(s) if s.len() > max => {
            let mut cut = max;
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            s.truncate(cut);
            true
        }
        Value::Array(a) => a.iter_mut().fold(false, |t, x| cap_strings(x, max) | t),
        Value::Object(o) => o.values_mut().fold(false, |t, x| cap_strings(x, max) | t),
        _ => false,
    }
}

#[derive(Deserialize)]
struct AttachmentPreview<'a> {
    #[serde(rename = "type", default)]
    attachment_type: Option<String>,
    #[serde(default, borrow)]
    content: Option<&'a RawValue>,
    #[serde(default, borrow)]
    stdout: Option<&'a RawValue>,
}

/// `(attachment.type, ≤ 1 KB preview)`: `content` (string or string list), else `stdout`, else
/// the JSON itself.
fn attachment_preview(raw: Option<&RawValue>) -> (String, String) {
    let Some(raw) = raw else {
        return (String::new(), String::new());
    };
    let p = serde_json::from_str::<AttachmentPreview<'_>>(raw.get()).ok();
    let kind = p
        .as_ref()
        .and_then(|p| p.attachment_type.clone())
        .unwrap_or_default();
    let text = p
        .as_ref()
        .and_then(|p| {
            let from_content = p.content.and_then(|c| {
                decode_str(Some(c)).or_else(|| {
                    serde_json::from_str::<Vec<Value>>(c.get())
                        .ok()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                })
            });
            from_content
                .filter(|t| !t.is_empty())
                .or_else(|| decode_str(p.stdout).filter(|t| !t.is_empty()))
        })
        .unwrap_or_else(|| raw.get().to_owned());
    (kind, cap_text(&text, PREVIEW_BYTES, usize::MAX).0)
}

/// Keys left out of a system entry's fallback preview.
const ENVELOPE_KEYS: &[&str] = &[
    "type",
    "subtype",
    "uuid",
    "parentUuid",
    "logicalParentUuid",
    "sessionId",
    "cwd",
    "version",
    "gitBranch",
    "timestamp",
    "userType",
    "entrypoint",
    "isSidechain",
    "isMeta",
    "level",
    "slug",
    "forkedFrom",
    "content",
];

/// ≤ 1 KB preview of a system entry: its `content` string, else its non-envelope fields as JSON.
fn system_text(ctx: &RenderCtx<'_>, i: usize, e: &RawEntry) -> String {
    if let Some(t) = decode_str(e.content.as_deref()) {
        return cap_text(&t, PREVIEW_BYTES, usize::MAX).0;
    }
    let (path, offset) = ctx.skeleton.location(ctx.files, i);
    let Some(line) = raw_line(path, offset) else {
        return String::new();
    };
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&line) else {
        return String::new();
    };
    for k in ENVELOPE_KEYS {
        map.remove(*k);
    }
    if map.is_empty() {
        return String::new();
    }
    cap_text(&Value::Object(map).to_string(), PREVIEW_BYTES, usize::MAX).0
}

/// Display text of a system entry that carries nothing beyond its envelope (e.g. `agents_killed`).
fn empty_system_text(subtype: &str) -> String {
    match subtype {
        "agents_killed" => "已终止所有后台 agent".to_owned(),
        "" => "未知系统记录".to_owned(),
        other => other.to_owned(),
    }
}

/// The source line at `offset` (read-only positional reads; for rare generic views only).
pub fn raw_line(path: &Path, offset: u64) -> Option<String> {
    use std::os::unix::fs::FileExt;
    const CHUNK: usize = 64 * 1024;
    const MAX: usize = 64 * 1024 * 1024;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; CHUNK];
    let mut pos = offset;
    loop {
        let n = f.read_at(&mut chunk, pos).ok()?;
        if n == 0 {
            break;
        }
        if let Some(nl) = memchr::memchr(b'\n', &chunk[..n]) {
            buf.extend_from_slice(&chunk[..nl]);
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        pos += n as u64;
        if buf.len() > MAX {
            return None;
        }
    }
    String::from_utf8(buf).ok()
}

/// `message.content[index]` of the source line at `offset`, as JSON.
fn raw_block(path: &Path, offset: u64, index: usize) -> Option<String> {
    let line = raw_line(path, offset)?;
    let v: Value = serde_json::from_str(&line).ok()?;
    Some(v.get("message")?.get("content")?.get(index)?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_text_by_bytes_and_lines() {
        let many = (0..100)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let (t, tr) = cap_text(&many, RESULT_BYTES, RESULT_LINES);
        assert!(tr);
        assert_eq!(t.lines().count(), RESULT_LINES);
        let (t, tr) = cap_text(&"é".repeat(2000), RESULT_BYTES, RESULT_LINES);
        assert!(tr && t.len() <= RESULT_BYTES);
        assert_eq!(cap_text("short", 10, 2), ("short".to_owned(), false));
    }

    #[test]
    fn caps_json_strings_keeps_valid_json() {
        let raw = format!(r#"{{"a":"{}","b":1}}"#, "x".repeat(5000));
        let (j, tr) = cap_json(&raw, INPUT_STRING_BYTES);
        assert!(tr);
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["a"].as_str().unwrap().len(), INPUT_STRING_BYTES);
        assert_eq!(v["b"], 1);
    }

    #[test]
    fn extra_json_drops_largest_fields() {
        let raw = format!(
            r#"{{"stdout":"{}","stderr":"oops","interrupted":false,"other":1}}"#,
            "y".repeat(20_000)
        );
        let raw = RawValue::from_string(raw).unwrap();
        let j = extra_json(Some(&raw), "Bash").unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        assert!(v.get("stdout").is_none());
        assert_eq!(v["stderr"], "oops");
        assert!(v.get("other").is_none());
        assert!(extra_json(Some(&raw), "Read").is_none());
    }
}
