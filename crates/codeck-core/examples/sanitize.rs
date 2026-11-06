//! Fixture sanitizer: turns a real Claude Code transcript (`.jsonl`) or sidecar (`.json`)
//! into a shareable one. Only structure survives; free text becomes deterministic filler.
//!
//! Usage:
//!   cargo run -p codeck-core --example sanitize -- <input> [--lines 1-50,70-80]
//!       [--uuids a,b,c | --uuid-file path] [--cwd /Users/dev/code/name] [--cap 160]
//!
//! Output goes to stdout, one JSON document per line. Lines that do not parse are skipped
//! with a warning on stderr. Rules:
//! - keys in `KEEP_KEYS` keep their string values when identifier-like, i.e. no whitespace and at most 200 bytes
//!   (paths rewritten to `/Users/dev`);
//! - every other string becomes filler (same line structure, similar length, capped);
//! - `signature` is dropped, base64 image data becomes a 1x1 PNG;
//! - `cwd` is replaced by `--cwd`, `persistedOutputPath` keeps only its basename;
//! - text inside `<persisted-output>` and a few structural tags keeps the tags and ids.

use serde_json::{Map, Value};
use std::collections::HashSet;
use std::io::Write;

const KEEP_KEYS: &[&str] = &[
    "type",
    "subtype",
    "uuid",
    "parentUuid",
    "logicalParentUuid",
    "sessionId",
    "session_id",
    "timestamp",
    "id",
    "tool_use_id",
    "toolUseId",
    "toolUseID",
    "sourceToolUseID",
    "sourceToolAssistantUUID",
    "agentId",
    "agent_id",
    "taskId",
    "task_id",
    "name",
    "model",
    "role",
    "is_error",
    "isMeta",
    "isSidechain",
    "isCompactSummary",
    "leafUuid",
    "forkedFrom",
    "continuedInSessionId",
    "sessionKind",
    "messageUuid",
    "messageId",
    "runId",
    "status",
    "state",
    "kind",
    "media_type",
    "version",
    "usage",
    "stop_reason",
    "origin",
    "parentAgentId",
    "spawnDepth",
    "agentType",
    "taskKind",
    "isFork",
    "entrypoint",
    "userType",
    "permissionMode",
    "promptSource",
    "promptId",
    "requestId",
    "operation",
    "mode",
    "level",
    "hookEvent",
    "service_tier",
    "workflowName",
    "phaseIndex",
    "teamName",
    "taskType",
    "label",
];
const PATH_KEYS: &[&str] = &["persistedOutputPath", "transcriptDir", "outputFile"];
const KEEP_TAG_CONTENT: &[&str] = &["task-id", "tool-use-id", "status", "command-name"];
const WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "exercitation",
    "ullamco",
    "laboris",
    "nisi",
    "aliquip",
];
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

struct Cfg {
    cwd: String,
    cap: usize,
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Deterministic filler of about `len` chars (same input, same output, so verbatim resends stay equal).
fn filler(seed: &str, len: usize, cap: usize) -> String {
    let len = len.min(cap);
    let mut x = fnv(seed) | 1;
    let mut out = String::new();
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(WORDS[(x % WORDS.len() as u64) as usize]);
    }
    out.truncate(len);
    out
}

fn rewrite_paths(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("/Users/") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 7..];
        let end = after
            .find(|c: char| c == '/' || c.is_whitespace() || c == '"' || c == '\'' || c == '<')
            .unwrap_or(after.len());
        out.push_str("/Users/dev");
        rest = &after[end..];
    }
    out.push_str(rest);
    // also scrub the invoking user's name where it appears outside a /Users/<name> path
    match std::env::var("USER") {
        Ok(u) if u.len() > 2 => out.replace(&u, "dev"),
        _ => out,
    }
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Replace text segments by filler line by line; keep simple `<tag>`s and the content of structural tags.
fn scramble(s: &str, cfg: &Cfg) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.starts_with("<persisted-output>") {
        return persisted(s, cfg);
    }
    let mut out = String::new();
    let mut seg = String::new();
    let mut keep_next = false;
    let mut chars = s.char_indices().peekable();
    let flush = |seg: &mut String, out: &mut String, keep: bool| {
        if seg.is_empty() {
            return;
        }
        if keep {
            out.push_str(&rewrite_paths(seg));
        } else {
            out.push_str(&scramble_text(seg, cfg));
        }
        seg.clear();
    };
    while let Some((i, c)) = chars.next() {
        if c == '<'
            && let Some(j) = s[i..].find('>')
        {
            let inner = &s[i + 1..i + j];
            let name = inner.trim_start_matches('/');
            let simple = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
            if simple {
                flush(&mut seg, &mut out, keep_next);
                out.push_str(&s[i..i + j + 1]);
                keep_next = !inner.starts_with('/') && KEEP_TAG_CONTENT.contains(&name);
                while chars.peek().is_some_and(|&(k, _)| k <= i + j) {
                    chars.next();
                }
                continue;
            }
        }
        seg.push(c);
    }
    flush(&mut seg, &mut out, keep_next);
    out
}

fn scramble_text(seg: &str, cfg: &Cfg) -> String {
    let lines: Vec<&str> = seg.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        if n >= 6 {
            break;
        }
        let t = line.trim();
        let lead = &line[..line.len() - line.trim_start().len()];
        let lead: String = lead.chars().take(8).collect();
        let body = if t.is_empty() {
            String::new()
        } else {
            filler(t, t.chars().count(), cfg.cap)
        };
        out.push(format!("{lead}{body}"));
    }
    out.join("\n")
}

fn persisted(s: &str, cfg: &Cfg) -> String {
    let size = s
        .find("Output too large (")
        .and_then(|i| s[i + 18..].find(')').map(|j| &s[i + 18..i + 18 + j]))
        .unwrap_or("10KB");
    let base = s
        .find("saved to: ")
        .map(|i| s[i + 10..].lines().next().unwrap_or(""))
        .map(|p| basename(p.trim()).to_string())
        .unwrap_or_else(|| "output.txt".into());
    let preview = filler(s, 120, cfg.cap);
    format!(
        "<persisted-output>\nOutput too large ({size}). Full output saved to: /Users/dev/.claude/tool-results/{base}\n\nPreview (first 2KB):\n{preview}\n...\n</persisted-output>"
    )
}

fn walk(key: Option<&str>, v: &Value, keep: bool, cfg: &Cfg, sibling_b64: bool) -> Value {
    match v {
        Value::String(s) => {
            if matches!(key, Some("data") | Some("base64")) && (sibling_b64 || s.len() > 64) {
                return Value::String(PNG_1X1.into());
            }
            match key {
                Some("cwd") => Value::String(cfg.cwd.clone()),
                Some("gitBranch") => Value::String("main".into()),
                Some(k) if PATH_KEYS.contains(&k) => {
                    Value::String(format!("/Users/dev/.claude/tool-results/{}", basename(s)))
                }
                // structural keys keep identifier-like values only; prose under a generic key (e.g. `role`) is scrambled
                _ if keep && s.len() <= 200 && !s.contains(char::is_whitespace) => {
                    Value::String(rewrite_paths(s))
                }
                _ => Value::String(scramble(s, cfg)),
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| walk(key, x, keep, cfg, false)).collect()),
        Value::Object(m) => {
            let b64 = m.get("type").and_then(Value::as_str) == Some("base64");
            let mut out = Map::new();
            for (k, x) in m {
                if k == "signature" {
                    continue;
                }
                let keep_here = keep || KEEP_KEYS.contains(&k.as_str());
                out.insert(safe_key(k), walk(Some(k), x, keep_here, cfg, b64));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Map keys can carry user text (question texts, file paths); only identifier-like keys survive.
fn safe_key(k: &str) -> String {
    let ok = k.len() <= 60
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '@'));
    if ok {
        k.to_string()
    } else {
        format!("key_{:08x}", fnv(k) as u32)
    }
}

fn parse_ranges(spec: &str) -> Vec<(usize, usize)> {
    spec.split(',')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('-') {
            Some((a, b)) => (
                a.parse().expect("range start"),
                b.parse().expect("range end"),
            ),
            None => {
                let n = p.parse().expect("line number");
                (n, n)
            }
        })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect(
        "usage: sanitize <input> [--lines a-b,c] [--uuids a,b] [--uuid-file f] [--cwd p] [--cap n]",
    );
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut uuids: Option<HashSet<String>> = None;
    let mut cfg = Cfg {
        cwd: "/Users/dev/code/app".into(),
        cap: 160,
    };
    while let Some(a) = args.next() {
        let val = args.next().unwrap_or_default();
        match a.as_str() {
            "--lines" => ranges = parse_ranges(&val),
            "--uuids" => uuids = Some(val.split(',').map(str::to_string).collect()),
            "--uuid-file" => {
                let text = std::fs::read_to_string(&val).expect("uuid file");
                uuids = Some(text.split_whitespace().map(str::to_string).collect());
            }
            "--cwd" => cfg.cwd = val,
            "--cap" => cfg.cap = val.parse().expect("cap"),
            other => panic!("unknown flag {other}"),
        }
    }
    let text = std::fs::read_to_string(&input).expect("read input");
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if input.ends_with(".json") {
        let v: Value = serde_json::from_str(&text).expect("json input");
        writeln!(out, "{}", walk(None, &v, false, &cfg, false)).unwrap();
        return;
    }
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if !ranges.is_empty() && !ranges.iter().any(|&(a, b)| n >= a && n <= b) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            eprintln!("line {n}: not JSON, skipped");
            continue;
        };
        if let Some(allow) = &uuids {
            let id = v
                .get("uuid")
                .or_else(|| v.get("leafUuid"))
                .and_then(Value::as_str);
            if id.is_some_and(|u| !allow.contains(u)) {
                continue;
            }
        }
        writeln!(out, "{}", walk(None, &v, false, &cfg, false)).unwrap();
    }
}
