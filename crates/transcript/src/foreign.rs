//! Helpers shared by the adapters of other agents' transcripts (`codex`, `pi`): finding files,
//! splitting complete lines, deterministic ids, and emitting Claude Code entries. No agent's format
//! is known here.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::parse::{EntryRecord, ParsedChunk};
use crate::raw::RawEntry;

/// A UUID-shaped id (version 5 / RFC 4122 layout) hashed from `(tag, id, a, b)`: the same input
/// gives the same id on every read, so node ids, anchors and expansion state survive a re-read.
pub(crate) fn derived_uuid(tag: &[u8], id: &str, a: u64, b: u32) -> String {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013b;
    let mut h = OFFSET;
    let mut feed = |bytes: &[u8]| {
        for &x in bytes {
            h ^= u128::from(x);
            h = h.wrapping_mul(PRIME);
        }
    };
    feed(tag);
    feed(&(id.len() as u64).to_le_bytes());
    feed(id.as_bytes());
    feed(&a.to_le_bytes());
    feed(&b.to_le_bytes());

    let mut x = h.to_be_bytes();
    x[6] = (x[6] & 0x0f) | 0x50; // version 5
    x[8] = (x[8] & 0x3f) | 0x80; // RFC 4122 variant
    let hex: String = x.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// An id that is safe to match against file names: ASCII letters, digits, `-` and `_`.
pub(crate) fn is_plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn entries(dir: &Path, want_dir: bool) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() == want_dir))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// Subdirectories of `dir`, sorted by name; empty when it is missing.
pub(crate) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    entries(dir, true)
}

/// Files (not directories) in `dir`, sorted by name; empty when it is missing.
pub(crate) fn files(dir: &Path) -> Vec<PathBuf> {
    entries(dir, false)
}

/// The whole file, opened read-only.
pub(crate) fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    File::open(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

/// One complete (`\n`-terminated) line.
pub(crate) struct Line<'a> {
    /// 1-based.
    pub line_no: u32,
    /// Byte offset of the line start.
    pub offset: u64,
    pub bytes: &'a [u8],
}

/// The complete lines of `buf` (a trailing partial line is left for the next read), the offset just
/// past the last `\n`, and the `\n` count.
pub(crate) fn lines(buf: &[u8]) -> (Vec<Line<'_>>, u64, u32) {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut n = 0u32;
    for (i, b) in buf.iter().enumerate() {
        if *b != b'\n' {
            continue;
        }
        n += 1;
        let bytes = &buf[start..i];
        if !bytes.iter().all(u8::is_ascii_whitespace) {
            out.push(Line {
                line_no: n,
                offset: start as u64,
                bytes,
            });
        }
        start = i + 1;
    }
    (out, start as u64, n)
}

/// Builds Claude Code entries for one adapted file: ids, chaining, timestamps, failed lines.
pub(crate) struct Emitter {
    tag: &'static [u8],
    session_id: String,
    records: Vec<EntryRecord>,
    last: Option<String>,
    line_no: u32,
    offset: u64,
    timestamp: Option<String>,
    sub: u32,
    failed: u32,
    first_error: Option<String>,
}

impl Emitter {
    pub(crate) fn new(tag: &'static [u8], session_id: &str) -> Self {
        Emitter {
            tag,
            session_id: session_id.to_owned(),
            records: Vec::new(),
            last: None,
            line_no: 0,
            offset: 0,
            timestamp: None,
            sub: 0,
            failed: 0,
            first_error: None,
        }
    }

    /// Starts the entries of one source line.
    pub(crate) fn at(&mut self, line: &Line<'_>, timestamp: Option<&str>) {
        self.line_no = line.line_no;
        self.offset = line.offset;
        self.timestamp = timestamp.map(str::to_owned);
        self.sub = 0;
    }

    /// An id for a source item that has none, stable across reads.
    pub(crate) fn fresh_id(&self) -> String {
        derived_uuid(self.tag, &self.session_id, self.offset, u32::MAX - self.sub)
    }

    /// The last entry emitted.
    pub(crate) fn last(&self) -> Option<String> {
        self.last.clone()
    }

    pub(crate) fn fail(&mut self, err: impl std::fmt::Display) {
        self.failed += 1;
        if self.first_error.is_none() {
            self.first_error = Some(format!("第 {} 行：{err}", self.line_no));
        }
    }

    /// Emits `obj` (a Claude Code entry without ids) after the last entry.
    pub(crate) fn push(&mut self, obj: Map<String, Value>) -> String {
        let parent = self.last.clone();
        self.push_under(obj, parent)
    }

    /// Emits `obj` under `parent` (a uuid emitted before, or none for a root).
    pub(crate) fn push_under(
        &mut self,
        mut obj: Map<String, Value>,
        parent: Option<String>,
    ) -> String {
        let uuid = derived_uuid(self.tag, &self.session_id, self.offset, self.sub);
        self.sub += 1;
        obj.insert("uuid".into(), Value::String(uuid.clone()));
        if let Some(p) = parent {
            obj.insert("parentUuid".into(), Value::String(p));
        }
        obj.insert("sessionId".into(), Value::String(self.session_id.clone()));
        if let Some(ts) = &self.timestamp {
            obj.insert("timestamp".into(), Value::String(ts.clone()));
        }
        let parsed = serde_json::to_vec(&Value::Object(obj))
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<RawEntry>(&b).map_err(|e| e.to_string()));
        match parsed {
            Ok(entry) => self.records.push(EntryRecord {
                line_no: self.line_no,
                offset: self.offset,
                entry,
            }),
            Err(e) => self.fail(e),
        }
        self.last = Some(uuid.clone());
        uuid
    }

    pub(crate) fn finish(
        self,
        consumed_to: u64,
        newlines: u32,
        file_len: u64,
    ) -> (ParsedChunk, u32) {
        (
            ParsedChunk {
                records: self.records,
                consumed_to,
                failed: self.failed,
                first_error: self.first_error,
                capped: false,
                file_len,
            },
            newlines,
        )
    }
}

/// A `user` entry with `content` (a string or Claude Code content blocks).
pub(crate) fn user(content: Value) -> Map<String, Value> {
    entry("user", json!({ "role": "user", "content": content }))
}

/// An `assistant` entry with Claude Code content blocks.
pub(crate) fn assistant(id: &str, model: Option<&str>, blocks: Vec<Value>) -> Map<String, Value> {
    let mut msg = Map::new();
    msg.insert("role".into(), "assistant".into());
    msg.insert("id".into(), Value::String(format!("msg_{id}")));
    if let Some(m) = model {
        msg.insert("model".into(), Value::String(m.to_owned()));
    }
    msg.insert("content".into(), Value::Array(blocks));
    entry("assistant", Value::Object(msg))
}

/// A `user` entry holding one tool result; `extra` is Claude Code's `toolUseResult`.
pub(crate) fn tool_result(
    tool_use_id: &str,
    text: &str,
    is_error: bool,
    extra: Option<Value>,
) -> Map<String, Value> {
    let mut obj = user(json!([{
        "type": "tool_result",
        "tool_use_id": tool_use_id,
        "content": text,
        "is_error": is_error,
    }]));
    if let Some(x) = extra {
        obj.insert("toolUseResult".into(), x);
    }
    obj
}

/// A system note (`level`: `info` or `error`), folded like Claude Code's.
pub(crate) fn system(level: &str, text: &str) -> Map<String, Value> {
    let mut obj = Map::new();
    obj.insert("type".into(), "system".into());
    obj.insert("subtype".into(), "informational".into());
    obj.insert("level".into(), Value::String(level.to_owned()));
    obj.insert("content".into(), Value::String(text.to_owned()));
    obj
}

/// An entry of a type assembly does not know: shown as unrecognized, with the source line.
pub(crate) fn unknown(kind: &str) -> Map<String, Value> {
    let mut obj = Map::new();
    obj.insert("type".into(), Value::String(kind.to_owned()));
    obj
}

fn entry(kind: &str, message: Value) -> Map<String, Value> {
    let mut obj = Map::new();
    obj.insert("type".into(), Value::String(kind.to_owned()));
    obj.insert("message".into(), message);
    obj
}

/// A data URL (`data:<type>;base64,<data>`) as a Claude Code image block.
pub(crate) fn image_from_data_url(url: &str) -> Option<Value> {
    let rest = url.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(";base64,")?;
    Some(image(media_type, data))
}

/// A Claude Code image block.
pub(crate) fn image(media_type: &str, data: &str) -> Value {
    json!({ "type": "image", "source": { "type": "base64", "media_type": media_type, "data": data } })
}

/// A unified diff's hunks as Claude Code's `structuredPatch` (`oldStart`, `oldLines`, `newStart`,
/// `newLines`, `lines` with their `' '` / `-` / `+` prefix); file headers are skipped.
pub(crate) fn structured_patch(diff: &str) -> Vec<Value> {
    let mut hunks: Vec<Value> = Vec::new();
    let mut cur: Option<(u64, u64, u64, u64, Vec<String>)> = None;
    let flush = |cur: &mut Option<(u64, u64, u64, u64, Vec<String>)>, hunks: &mut Vec<Value>| {
        if let Some((os, ol, ns, nl, lines)) = cur.take() {
            hunks.push(json!({ "oldStart": os, "oldLines": ol, "newStart": ns, "newLines": nl, "lines": lines }));
        }
    };
    for line in diff.lines() {
        if let Some(h) = line.strip_prefix("@@ ") {
            flush(&mut cur, &mut hunks);
            let mut parts = h.split_whitespace();
            let (Some(old), Some(new)) = (parts.next(), parts.next()) else {
                continue;
            };
            let range = |s: &str, sign: char| -> Option<(u64, u64)> {
                let s = s.strip_prefix(sign)?;
                match s.split_once(',') {
                    Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
                    None => Some((s.parse().ok()?, 1)),
                }
            };
            if let (Some((os, ol)), Some((ns, nl))) = (range(old, '-'), range(new, '+')) {
                cur = Some((os, ol, ns, nl, Vec::new()));
            }
        } else if let Some((.., lines)) = cur.as_mut()
            && (line.starts_with(' ')
                || line.starts_with('-')
                || line.starts_with('+')
                || line.is_empty())
        {
            lines.push(if line.is_empty() {
                " ".to_owned()
            } else {
                line.to_owned()
            });
        }
    }
    flush(&mut cur, &mut hunks);
    hunks
}
