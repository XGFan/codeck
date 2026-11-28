//! Read and translate agy (Antigravity CLI) Worker transcripts.
//!
//! agy steps become Claude Code [`EntryRecord`]s for shared transcript assembly and rendering.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value, json};

use crate::known;
use crate::parse::EntryRecord;
use crate::raw::{RawEntry, lenient};

// ---- on-disk layout ----

/// `<root>/brain`: one subdirectory per conversation id.
pub fn brain_dir(root: &Path) -> PathBuf {
    root.join("brain")
}

/// Whether `id` can be a conversation id: one plain path component (no separator, not `.` or
/// `..`). Ids from file names and command lines go through it before they are joined into a path.
pub fn is_conversation_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', '\0'])
}

/// `<root>/brain/<id>/.system_generated/logs/transcript_full.jsonl`.
pub fn transcript_path(root: &Path, id: &str) -> PathBuf {
    brain_dir(root)
        .join(id)
        .join(".system_generated")
        .join("logs")
        .join("transcript_full.jsonl")
}

/// Finds the transcript file for a valid conversation id.
pub fn find_transcript(root: &Path, id: &str) -> Option<PathBuf> {
    if !is_conversation_id(id) {
        return None;
    }
    let path = transcript_path(root, id);
    path.is_file().then_some(path)
}

/// Conversation id of a transcript path under `root`, if it matches [`transcript_path`].
pub fn session_of_transcript(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(brain_dir(root)).ok()?;
    let mut comps = rel.components();
    let id = match comps.next()? {
        Component::Normal(s) => s.to_str()?,
        _ => return None,
    };
    if !is_conversation_id(id) {
        return None;
    }
    let expected = [".system_generated", "logs", "transcript_full.jsonl"];
    for want in expected {
        match comps.next()? {
            Component::Normal(s) if s == want => {}
            _ => return None,
        }
    }
    comps.next().is_none().then(|| id.to_owned())
}

// ---- transcript parsing ----

/// Parsed agy `transcript_full.jsonl`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AgyFile {
    /// Winning steps (last line wins per `step_index`), sorted ascending by `index`.
    pub steps: Vec<Step>,
    /// Complete lines that could not be parsed as a step with a `step_index`.
    pub failed: u32,
    pub first_error: Option<String>,
    /// Byte offset just past the last complete `\n` (trailing partial line is not read).
    pub consumed_to: u64,
    /// Number of complete `\n`-terminated physical lines scanned.
    pub lines: u32,
}

/// One agy step; every JSON field except `step_index` is optional.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Step {
    pub index: u64,
    /// Byte offset of the winning line in the file.
    pub offset: u64,
    /// 1-based line number of the winning line in the file.
    pub line_no: u32,
    pub step_type: Option<String>,
    pub source: Option<String>,
    pub status: Option<String>,
    pub created_at: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub thinking: Option<String>,
    pub tool_calls: Vec<AgyToolCall>,
    pub content: Option<String>,
    pub error: Option<String>,
}

/// One element of `tool_calls` on a `PLANNER_RESPONSE` step.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AgyToolCall {
    pub name: Option<String>,
    pub args: Option<Value>,
}

#[derive(Deserialize)]
struct RawStep {
    #[serde(default, deserialize_with = "lenient_u64")]
    step_index: Option<u64>,
    #[serde(rename = "type", default, deserialize_with = "lenient::string")]
    step_type: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    source: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    status: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    created_at: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "lenient_u64")]
    input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "lenient_u64")]
    output_tokens: Option<u64>,
    #[serde(default, deserialize_with = "lenient_u64")]
    cache_read_tokens: Option<u64>,
    #[serde(default, deserialize_with = "lenient::string")]
    thinking: Option<String>,
    #[serde(default, deserialize_with = "lenient_tool_calls")]
    tool_calls: Vec<AgyToolCall>,
    #[serde(default, deserialize_with = "lenient::string")]
    content: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct RawToolCall {
    #[serde(default, deserialize_with = "lenient::string")]
    name: Option<String>,
    #[serde(default)]
    args: Option<Value>,
}

/// Parses complete (`\n`-terminated) lines of an agy `transcript_full.jsonl` buffer.
///
/// Blank lines are skipped silently. A trailing line without `\n` is left unread (`consumed_to`
/// stops right after the last `\n`). When the same `step_index` appears multiple times, the last
/// occurrence wins (including its `offset` and `line_no`); `steps` are sorted by `index`.
pub fn parse(buf: &[u8]) -> AgyFile {
    let mut by_index: BTreeMap<u64, Step> = BTreeMap::new();
    let mut failed = 0u32;
    let mut first_error: Option<String> = None;
    let mut start = 0usize;
    let mut lines = 0u32;

    for (idx, nl) in memchr::memchr_iter(b'\n', buf).enumerate() {
        let line_no = idx as u32 + 1;
        lines = line_no;
        let offset = start as u64;
        let line = buf[start..nl].trim_ascii();
        start = nl + 1;
        if line.is_empty() {
            continue;
        }
        let parsed = if line[0] == b'{' {
            match serde_json::from_slice::<RawStep>(line) {
                Ok(raw) => match raw.step_index {
                    Some(index) => Ok(Step {
                        index,
                        offset,
                        line_no,
                        step_type: raw.step_type,
                        source: raw.source,
                        status: raw.status,
                        created_at: raw.created_at,
                        model: raw.model,
                        input_tokens: raw.input_tokens,
                        output_tokens: raw.output_tokens,
                        cache_read_tokens: raw.cache_read_tokens,
                        thinking: raw.thinking,
                        tool_calls: raw.tool_calls,
                        content: raw.content,
                        error: raw.error,
                    }),
                    None => Err("缺少 step_index".to_owned()),
                },
                Err(e) => Err(e.to_string()),
            }
        } else {
            Err("不是 JSON 对象".to_owned())
        };
        match parsed {
            Ok(step) => {
                by_index.insert(step.index, step);
            }
            Err(e) => {
                failed += 1;
                if first_error.is_none() {
                    first_error = Some(format!("字节偏移 {offset}：{e}"));
                }
            }
        }
    }

    AgyFile {
        steps: by_index.into_values().collect(),
        failed,
        first_error,
        consumed_to: start as u64,
        lines,
    }
}

/// Reads and parses an agy `transcript_full.jsonl` file (strictly read-only).
pub fn read(path: &Path) -> io::Result<AgyFile> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(parse(&buf))
}

// ---- adaptation to Claude Code RawEntry ----

/// Result of translating an [`AgyFile`] into Claude Code [`EntryRecord`]s.
#[derive(Debug, Default)]
pub struct Adapted {
    pub records: Vec<EntryRecord>,
}

/// Translates `file` into a straight `parentUuid`-chained sequence of Claude Code [`EntryRecord`]s.
///
/// Each record is constructed as Claude Code JSON and deserialized through [`RawEntry`] for downstream assembly.
pub fn adapt(conv_id: &str, file: &AgyFile) -> Adapted {
    let mut records = Vec::with_capacity(file.steps.len());
    let mut prev_uuid: Option<String> = None;
    let mut pending_calls: VecDeque<String> = VecDeque::new();

    for step in &file.steps {
        let uuid = make_uuid(conv_id, step.index, 0);
        let mut obj = Map::new();
        obj.insert("uuid".to_owned(), Value::String(uuid.clone()));
        if let Some(parent) = &prev_uuid {
            obj.insert("parentUuid".to_owned(), Value::String(parent.clone()));
        }
        obj.insert("sessionId".to_owned(), Value::String(conv_id.to_owned()));
        if let Some(ts) = &step.created_at {
            obj.insert("timestamp".to_owned(), Value::String(ts.clone()));
        }

        match step.step_type.as_deref() {
            Some("USER_INPUT") => {
                pending_calls.clear();
                let text = step
                    .content
                    .as_deref()
                    .map(extract_user_request)
                    .unwrap_or("");
                obj.insert("type".to_owned(), Value::String("user".to_owned()));
                obj.insert(
                    "message".to_owned(),
                    json!({
                        "role": "user",
                        "content": text,
                    }),
                );
            }
            Some("PLANNER_RESPONSE") => {
                pending_calls.clear();
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(thinking) = step.thinking.as_deref().filter(|t| !t.is_empty()) {
                    blocks.push(json!({
                        "type": "thinking",
                        "thinking": thinking,
                    }));
                }
                if let Some(content) = step.content.as_deref().filter(|c| !c.is_empty()) {
                    blocks.push(json!({
                        "type": "text",
                        "text": content,
                    }));
                }
                for (k, tc) in step.tool_calls.iter().enumerate() {
                    let raw_name = tc.name.as_deref().unwrap_or("");
                    let tool_id = make_uuid(conv_id, step.index, k as u32 + 1);
                    pending_calls.push_back(tool_id.clone());
                    let (name, input) = translate_tool(raw_name, tc.args.as_ref());
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tool_id,
                        "name": name,
                        "input": input,
                    }));
                }

                let mut msg = Map::new();
                msg.insert("role".to_owned(), Value::String("assistant".to_owned()));
                msg.insert("id".to_owned(), Value::String(format!("msg_{uuid}")));
                if let Some(model) = &step.model {
                    msg.insert("model".to_owned(), Value::String(model.clone()));
                }
                msg.insert("content".to_owned(), Value::Array(blocks));
                if step.input_tokens.is_some()
                    || step.output_tokens.is_some()
                    || step.cache_read_tokens.is_some()
                {
                    let mut usage = Map::new();
                    if let Some(n) = step.input_tokens {
                        usage.insert("input_tokens".to_owned(), json!(n));
                    }
                    if let Some(n) = step.output_tokens {
                        usage.insert("output_tokens".to_owned(), json!(n));
                    }
                    if let Some(n) = step.cache_read_tokens {
                        usage.insert("cache_read_input_tokens".to_owned(), json!(n));
                    }
                    msg.insert("usage".to_owned(), Value::Object(usage));
                }

                obj.insert("type".to_owned(), Value::String("assistant".to_owned()));
                obj.insert("message".to_owned(), Value::Object(msg));
            }
            Some("GENERIC") => {
                let tool_use_id = pending_calls
                    .pop_front()
                    .unwrap_or_else(|| make_uuid(conv_id, step.index, 1));
                let stripped = step
                    .content
                    .as_deref()
                    .map(strip_timestamps_header)
                    .unwrap_or("");
                let text = if stripped.is_empty() {
                    step.error.as_deref().unwrap_or("")
                } else {
                    stripped
                };
                let is_error = step.status.as_deref() == Some("ERROR");
                obj.insert("type".to_owned(), Value::String("user".to_owned()));
                obj.insert(
                    "message".to_owned(),
                    json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": text,
                            "is_error": is_error,
                        }],
                    }),
                );
            }
            Some("SYSTEM_MESSAGE") => {
                let text = step
                    .content
                    .as_deref()
                    .map(unwrap_system_message)
                    .unwrap_or("");
                obj.insert("type".to_owned(), Value::String("system".to_owned()));
                obj.insert(
                    "subtype".to_owned(),
                    Value::String("informational".to_owned()),
                );
                obj.insert("level".to_owned(), Value::String("info".to_owned()));
                obj.insert("content".to_owned(), Value::String(text.to_owned()));
            }
            Some("ERROR_MESSAGE") => {
                let text = step
                    .error
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .or(step.content.as_deref())
                    .unwrap_or("");
                obj.insert("type".to_owned(), Value::String("system".to_owned()));
                obj.insert(
                    "subtype".to_owned(),
                    Value::String("informational".to_owned()),
                );
                obj.insert("level".to_owned(), Value::String("error".to_owned()));
                obj.insert("content".to_owned(), Value::String(text.to_owned()));
            }
            other => {
                let raw_type = other.unwrap_or("").to_owned();
                // Ensure the emitted entry type is also unrecognized by Claude Code assembly so it
                // renders as `NodeBody::Unknown` and reads back the raw line at `step.offset`.
                let emitted_type = if raw_type.is_empty() {
                    "agy_unknown".to_owned()
                } else if known::is_known_entry_type(&raw_type) {
                    format!("agy:{raw_type}")
                } else {
                    raw_type
                };
                obj.insert("type".to_owned(), Value::String(emitted_type));
            }
        }

        let bytes = serde_json::to_vec(&Value::Object(obj)).expect("serialize adapted entry");
        let entry: RawEntry = serde_json::from_slice(&bytes).expect("deserialize adapted RawEntry");
        records.push(EntryRecord {
            line_no: step.line_no,
            offset: step.offset,
            entry,
        });
        prev_uuid = Some(uuid);
    }

    Adapted { records }
}

/// Extracts the text between `<USER_REQUEST>` and the last `</USER_REQUEST>`, dropping trailing
/// `<ADDITIONAL_METADATA>` / `<USER_SETTINGS_CHANGE>`. Returns `content` unchanged when the tags
/// are absent.
fn extract_user_request(content: &str) -> &str {
    const OPEN: &str = "<USER_REQUEST>";
    const CLOSE: &str = "</USER_REQUEST>";
    let Some(start) = content.find(OPEN) else {
        return content;
    };
    let rest = &content[start + OPEN.len()..];
    let Some(end) = rest.rfind(CLOSE) else {
        return content;
    };
    rest[..end].trim()
}

/// Strips leading `Created At:` / `Completed At:` lines (which agy may repeat twice) from a
/// `GENERIC` tool result.
fn strip_timestamps_header(mut s: &str) -> &str {
    loop {
        let end = s.find('\n').unwrap_or(s.len());
        let line = s[..end].trim_end_matches('\r');
        if line.starts_with("Created At:") || line.starts_with("Completed At:") {
            s = if end < s.len() { &s[end + 1..] } else { "" };
        } else {
            break;
        }
    }
    s
}

/// Unwraps agy's `<SYSTEM_MESSAGE>` block and its leading preamble sentence ("The following is a
/// `<SYSTEM_MESSAGE>` not actually sent by the user..."). Only a `<SYSTEM_MESSAGE>` tag starting a
/// line opens the block, so the mention inside the preamble is ignored.
fn unwrap_system_message(content: &str) -> &str {
    const OPEN: &str = "<SYSTEM_MESSAGE>";
    const CLOSE: &str = "</SYSTEM_MESSAGE>";
    let mut search_from = 0;
    let mut open_end = None;
    while let Some(rel) = content[search_from..].find(OPEN) {
        let at = search_from + rel;
        if at == 0 || content.as_bytes()[at - 1] == b'\n' {
            open_end = Some(at + OPEN.len());
            break;
        }
        search_from = at + OPEN.len();
    }
    let Some(after_open) = open_end else {
        return content.trim();
    };
    let rest = &content[after_open..];
    let body = match rest.rfind(CLOSE) {
        Some(end) => &rest[..end],
        None => rest,
    };
    body.trim()
}

/// Translates an agy tool call into `(name, input)` for Claude Code's renderers.
///
/// Translated tools carry `"agyOriginal": {"name", "args"}` inside `input`. Tools outside the
/// translation table or missing required arguments keep their original name and arguments.
fn translate_tool(raw_name: &str, raw_args: Option<&Value>) -> (String, Value) {
    let fallback = || {
        (
            raw_name.to_owned(),
            raw_args.cloned().unwrap_or_else(|| json!({})),
        )
    };
    let Some(args_obj) = raw_args.and_then(Value::as_object) else {
        return fallback();
    };

    let translated: Option<(&str, Map<String, Value>)> = match raw_name {
        "run_command" => args_obj
            .get("CommandLine")
            .and_then(Value::as_str)
            .map(|cmd| {
                let mut m = Map::new();
                m.insert("command".to_owned(), Value::String(cmd.to_owned()));
                if let Some(desc) = args_obj.get("toolSummary").and_then(Value::as_str) {
                    m.insert("description".to_owned(), Value::String(desc.to_owned()));
                }
                ("Bash", m)
            }),
        "read_file" => args_obj.get("Path").and_then(Value::as_str).map(|path| {
            let mut m = Map::new();
            m.insert("file_path".to_owned(), Value::String(path.to_owned()));
            let start = args_obj
                .get("StartLine")
                .and_then(Value::as_i64)
                .filter(|&n| n >= 0);
            let end = args_obj
                .get("EndLine")
                .and_then(Value::as_i64)
                .filter(|&n| n >= 0);
            if let Some(s) = start {
                m.insert("offset".to_owned(), json!(s));
                if let Some(e) = end
                    && e >= s
                {
                    m.insert("limit".to_owned(), json!(e - s + 1));
                }
            } else if let Some(e) = end
                && e >= 1
            {
                m.insert("limit".to_owned(), json!(e));
            }
            ("Read", m)
        }),
        "edit_file" => (|| {
            let path = args_obj.get("Path").and_then(Value::as_str)?;
            let target = args_obj.get("Target").and_then(Value::as_str)?;
            let replacement = args_obj.get("Replacement").and_then(Value::as_str)?;
            let mut m = Map::new();
            m.insert("file_path".to_owned(), Value::String(path.to_owned()));
            m.insert("old_string".to_owned(), Value::String(target.to_owned()));
            m.insert(
                "new_string".to_owned(),
                Value::String(replacement.to_owned()),
            );
            Some(("Edit", m))
        })(),
        "write_file" => (|| {
            let path = args_obj.get("Path").and_then(Value::as_str)?;
            let content = args_obj.get("Content").and_then(Value::as_str)?;
            let mut m = Map::new();
            m.insert("file_path".to_owned(), Value::String(path.to_owned()));
            m.insert("content".to_owned(), Value::String(content.to_owned()));
            Some(("Write", m))
        })(),
        "update_steps" => args_obj
            .get("steps")
            .and_then(Value::as_array)
            .and_then(|arr| {
                let mut todos = Vec::with_capacity(arr.len());
                for item in arr {
                    let Some(obj) = item.as_object() else {
                        continue;
                    };
                    let Some(title) = obj.get("title").and_then(Value::as_str) else {
                        continue;
                    };
                    let status = match obj.get("status").and_then(Value::as_str) {
                        Some("in_progress") => "in_progress",
                        Some("completed" | "done") => "completed",
                        _ => "pending",
                    };
                    todos.push(json!({
                        "content": title,
                        "status": status,
                    }));
                }
                if !arr.is_empty() && todos.is_empty() {
                    return None;
                }
                let mut m = Map::new();
                m.insert("todos".to_owned(), Value::Array(todos));
                Some(("TodoWrite", m))
            }),
        "search_web" => args_obj.get("query").and_then(Value::as_str).map(|query| {
            let mut m = Map::new();
            m.insert("query".to_owned(), Value::String(query.to_owned()));
            ("WebSearch", m)
        }),
        _ => None,
    };

    match translated {
        Some((name, mut map)) => {
            map.insert(
                "agyOriginal".to_owned(),
                json!({
                    "name": raw_name,
                    "args": raw_args.cloned().unwrap_or_else(|| json!({})),
                }),
            );
            (name.to_owned(), Value::Object(map))
        }
        None => fallback(),
    }
}

/// Deterministic UUID (version 5 / RFC 4122 layout) derived from `(conv_id, step_index, sub)`.
fn make_uuid(conv_id: &str, step_index: u64, sub: u32) -> String {
    crate::foreign::derived_uuid(b"codeck-agy-v1\0", conv_id, step_index, sub)
}

// ---- lenient deserializers for agy fields ----

fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Option<u64>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a non-negative integer")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(u64::try_from(v).ok())
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(self)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(None)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(None)
        }
    }
    d.deserialize_any(V)
}

struct LenientToolCall(Option<AgyToolCall>);

impl<'de> Deserialize<'de> for LenientToolCall {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw: Option<RawToolCall> = lenient::object(d)?;
        Ok(LenientToolCall(raw.map(|r| AgyToolCall {
            name: r.name,
            args: r.args,
        })))
    }
}

fn lenient_tool_calls<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<AgyToolCall>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Vec<AgyToolCall>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a tool_calls array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(LenientToolCall(tc)) = seq.next_element()? {
                out.extend(tc);
            }
            Ok(out)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(Vec::new())
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
            d.deserialize_any(self)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }
    d.deserialize_any(V)
}
