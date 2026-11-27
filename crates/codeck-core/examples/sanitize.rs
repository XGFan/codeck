//! Fixture sanitizer: turns a real Claude Code transcript (`.jsonl`) or sidecar (`.json`)
//! — or an Antigravity CLI (`--agy`) mini data root — into a shareable fixture.
//! Only structure survives; free text becomes deterministic filler.
//!
//! Usage (Claude Code):
//!   cargo run -p codeck-core --example sanitize -- <input> [--lines 1-50,70-80]
//!       [--uuids a,b,c | --uuid-file path] [--cwd /Users/dev/code/name] [--cap 160]
//!
//! Usage (Antigravity CLI):
//!   cargo run -p codeck-core --example sanitize -- --agy --source <agy_root> --out <dir>
//!       --ids <id1,id2,...> [--lock <id>] [--cap 160]
//!
//! Claude Code mode writes to stdout, one JSON document per line. Lines that do not parse are
//! skipped with a warning on stderr. Rules:
//! - keys in `KEEP_KEYS` keep their string values when identifier-like, i.e. no whitespace and at most 200 bytes
//!   (paths rewritten to `/Users/dev`);
//! - every other string becomes filler (same line structure, similar length, capped);
//! - `signature` is dropped, base64 image data becomes a 1x1 PNG;
//! - `cwd` is replaced by `--cwd`, `persistedOutputPath` keeps only its basename;
//! - text inside `<persisted-output>` and a few structural tags keeps the tags and ids.
//!
//! Agy mode reads `<source>/brain/<id>/.system_generated/logs/transcript_full.jsonl` and
//! `<source>/cache/conversation_metadata.json` read-only and writes a sanitized mini agy data root
//! into `--out` with deterministically remapped conversation IDs, `/Users/dev/...` paths
//! (`~/.haa` -> `/Users/dev/.haa`), preserved structural tags/headers/tool names/arg keys, and
//! an optional empty `presence/<remapped_id>.lock` file.

use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

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

const AGY_SYS_LEAD: &str = "The following is a <SYSTEM_MESSAGE> not actually sent by the user. It is provided by the system as important information to pay attention to.";
const AGY_KEEP_STEP_KEYS: &[&str] = &["source", "type", "status", "created_at", "model"];
const AGY_KEEP_META_KEYS: &[&str] = &["UpdatedAt", "AppDataDir", "ProjectID", "last_modified_time"];

fn remap_uuid(id: &str) -> String {
    let hi = fnv(id);
    let lo = fnv(&format!("{id}:agy-fixture"));
    let b0 = (hi >> 32) as u32;
    let b1 = ((hi >> 16) & 0xffff) as u16;
    let b2 = ((hi & 0x0fff) as u16) | 0x5000;
    let b3 = (((lo >> 48) & 0x3fff) as u16) | 0x8000;
    let b4 = lo & 0x0000_ffff_ffff_ffff;
    format!("{b0:08x}-{b1:04x}-{b2:04x}-{b3:04x}-{b4:012x}")
}

fn replace_ids(s: &str, id_map: &HashMap<String, String>) -> String {
    let mut out = s.to_string();
    for (orig, mapped) in id_map {
        out = out.replace(orig, mapped);
    }
    out
}

fn word_for(seed: &str) -> &'static str {
    WORDS[(fnv(seed) % WORDS.len() as u64) as usize]
}

fn sanitize_rel_segments(rest: &str) -> String {
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let mut out = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        let is_last = i + 1 == parts.len();
        let mapped = if is_last
            && let Some((stem, ext)) = part.rsplit_once('.')
            && !stem.is_empty()
            && !ext.is_empty()
            && ext.len() <= 8
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
        {
            format!("{}_{:04x}.{ext}", word_for(stem), fnv(part) as u16)
        } else {
            format!("{}_{:04x}", word_for(part), fnv(part) as u16)
        };
        out.push(mapped);
    }
    out.join("/")
}

/// Rewrite absolute/URI paths into `/Users/dev/...`, preserving `~/.haa` -> `/Users/dev/.haa`
/// and `.gemini/antigravity-cli` internal task/step paths while scrubbing usernames and repo names.
fn sanitize_agy_path(raw: &str, id_map: &HashMap<String, String>) -> String {
    let (prefix, path) = match raw.strip_prefix("file://") {
        Some(rest) => ("file://", rest),
        None => ("", raw),
    };
    if let Some(i) = path.find("/.gemini/antigravity-cli/") {
        let tail = replace_ids(&path[i + "/.gemini/antigravity-cli/".len()..], id_map);
        return format!("{prefix}/Users/dev/.gemini/antigravity-cli/{tail}");
    }
    if let Some(i) = path.find("/.haa") {
        let after = &path[i + "/.haa".len()..];
        if after.is_empty() || after == "/" {
            return format!("{prefix}/Users/dev/.haa");
        }
        if after == "/agy-hooks" {
            return format!("{prefix}/Users/dev/.haa/agy-hooks");
        }
        if let Some(wt_rest) = after.strip_prefix("/worktrees/") {
            let (wt, sub) = match wt_rest.split_once('/') {
                Some((w, s)) => (w, Some(s)),
                None => (wt_rest, None),
            };
            let mapped_wt = format!("wt-{:08x}", fnv(wt) as u32);
            return match sub {
                Some(s) if !s.is_empty() => format!(
                    "{prefix}/Users/dev/.haa/worktrees/{mapped_wt}/{}",
                    sanitize_rel_segments(s)
                ),
                _ => format!("{prefix}/Users/dev/.haa/worktrees/{mapped_wt}"),
            };
        }
        if let Some(rest) = after.strip_prefix('/') {
            return format!("{prefix}/Users/dev/.haa/{}", sanitize_rel_segments(rest));
        }
    }
    if path.starts_with('/') {
        return format!("{prefix}/Users/dev/code/ws-{:08x}", fnv(path) as u32);
    }
    sanitize_rel_segments(path)
}

fn scramble_block_body(seg: &str, cfg: &Cfg) -> String {
    let lead_nl = seg.starts_with('\n');
    let trail_nl = seg.len() > usize::from(lead_nl) && seg.ends_with('\n');
    let inner = seg
        .strip_prefix('\n')
        .unwrap_or(seg)
        .strip_suffix('\n')
        .unwrap_or(seg.strip_prefix('\n').unwrap_or(seg));
    let body = if inner.is_empty() {
        String::new()
    } else {
        scramble_text(inner, cfg)
    };
    let mut out = String::new();
    if lead_nl {
        out.push('\n');
    }
    out.push_str(&body);
    if trail_nl {
        out.push('\n');
    }
    out
}

fn sanitize_agy_user_input(s: &str, cfg: &Cfg) -> String {
    const TAGS: &[&str] = &[
        "<USER_REQUEST>",
        "</USER_REQUEST>",
        "<ADDITIONAL_METADATA>",
        "</ADDITIONAL_METADATA>",
        "<USER_SETTINGS_CHANGE>",
        "</USER_SETTINGS_CHANGE>",
    ];
    let mut out = String::new();
    let mut rest = s;
    let mut in_meta = false;
    while !rest.is_empty() {
        let next_tag = TAGS
            .iter()
            .filter_map(|&t| rest.find(t).map(|pos| (pos, t)))
            .min_by_key(|&(pos, _)| pos);
        let (chunk, tag) = match next_tag {
            Some((pos, t)) => (&rest[..pos], Some(t)),
            None => (rest, None),
        };
        if !chunk.is_empty() {
            if in_meta {
                let lines: Vec<String> = chunk
                    .split('\n')
                    .map(|l| {
                        if l.is_empty() || l.starts_with("The current local time is: ") {
                            l.to_string()
                        } else {
                            filler(l.trim(), l.trim().chars().count(), cfg.cap)
                        }
                    })
                    .collect();
                out.push_str(&lines.join("\n"));
            } else {
                out.push_str(&scramble_block_body(chunk, cfg));
            }
        }
        if let Some(t) = tag {
            out.push_str(t);
            in_meta = t == "<ADDITIONAL_METADATA>";
            rest = &rest[chunk.len() + t.len()..];
        } else {
            break;
        }
    }
    out
}

fn sanitize_agy_system_message(s: &str, id_map: &HashMap<String, String>, cfg: &Cfg) -> String {
    let (lead, rest) = if let Some(after_lead) = s.strip_prefix(AGY_SYS_LEAD) {
        let Some((between, after_open)) = after_lead.split_once("<SYSTEM_MESSAGE>") else {
            return format!("{AGY_SYS_LEAD}{}", scramble_block_body(after_lead, cfg));
        };
        let clean_between = if between.chars().all(char::is_whitespace) {
            between.to_string()
        } else {
            scramble_block_body(between, cfg)
        };
        (format!("{AGY_SYS_LEAD}{clean_between}"), after_open)
    } else if let Some((before, after_open)) = s.split_once("<SYSTEM_MESSAGE>") {
        (scramble_block_body(before, cfg), after_open)
    } else {
        return scramble_text(s, cfg);
    };
    let (inner, after_close) = match rest.rsplit_once("</SYSTEM_MESSAGE>") {
        Some((body, tail)) => (body, Some(tail)),
        None => (rest, None),
    };
    let mut body_out: Vec<String> = Vec::new();
    let mut scrambled_lines = 0usize;
    let lead_nl = inner.starts_with('\n');
    let trail_nl = inner.len() > usize::from(lead_nl) && inner.ends_with('\n');
    let trimmed = inner
        .strip_prefix('\n')
        .unwrap_or(inner)
        .strip_suffix('\n')
        .unwrap_or(inner.strip_prefix('\n').unwrap_or(inner));
    for line in trimmed.split('\n') {
        if line.starts_with("[Message] ")
            && let Some((hdr, content)) = line.split_once(" content=")
        {
            let clean_hdr = replace_ids(hdr, id_map);
            let clean_content = filler(content.trim(), content.trim().chars().count(), cfg.cap);
            body_out.push(format!("{clean_hdr} content={clean_content}"));
        } else if line.starts_with("Command exited with code ") {
            body_out.push(line.to_string());
        } else if let Some(uri) = line.strip_prefix("Log: ") {
            body_out.push(format!("Log: {}", sanitize_agy_path(uri.trim(), id_map)));
        } else if line.trim().is_empty() {
            if scrambled_lines < 6 {
                body_out.push(String::new());
            }
        } else if scrambled_lines < 6 {
            let t = line.trim();
            body_out.push(filler(t, t.chars().count(), cfg.cap));
            scrambled_lines += 1;
        }
    }
    let mut out = lead;
    out.push_str("<SYSTEM_MESSAGE>");
    if lead_nl {
        out.push('\n');
    }
    out.push_str(&body_out.join("\n"));
    if trail_nl {
        out.push('\n');
    }
    if let Some(tail) = after_close {
        out.push_str("</SYSTEM_MESSAGE>");
        if !tail.is_empty() {
            out.push_str(&scramble_block_body(tail, cfg));
        }
    }
    out
}

fn sanitize_agy_generic(s: &str, id_map: &HashMap<String, String>, cfg: &Cfg) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut body_lines = 0usize;
    for line in s.split('\n') {
        if line.starts_with("Created At: ")
            || line.starts_with("Completed At: ")
            || line.starts_with("Command exited with code ")
            || line == "Command output:"
        {
            out.push(line.to_string());
        } else if line.starts_with("Tool is running as a background task") {
            out.push(replace_ids(line, id_map));
        } else if let Some(desc) = line.strip_prefix("Task Description: ") {
            let t = desc.trim();
            out.push(format!(
                "Task Description: {}",
                filler(t, t.chars().count(), cfg.cap)
            ));
        } else if let Some(uri) = line.strip_prefix("Task logs are available at: ") {
            out.push(format!(
                "Task logs are available at: {}",
                sanitize_agy_path(uri.trim(), id_map)
            ));
        } else if let Some(rest) = line.strip_prefix(
            "The tool's output was truncated due to being too long. You can find the full output in a file at ",
        ) {
            let trimmed = rest.trim_end_matches('.');
            let dot = if rest.ends_with('.') { "." } else { "" };
            out.push(format!(
                "The tool's output was truncated due to being too long. You can find the full output in a file at {}{dot}",
                sanitize_agy_path(trimmed.trim(), id_map)
            ));
        } else if line.trim().is_empty() {
            if body_lines < 6 {
                out.push(String::new());
            }
        } else if body_lines < 6 {
            let t = line.trim();
            out.push(filler(t, t.chars().count(), cfg.cap));
            body_lines += 1;
        }
    }
    out.join("\n")
}

fn sanitize_agy_tool_args(
    key: Option<&str>,
    v: &Value,
    id_map: &HashMap<String, String>,
    cfg: &Cfg,
) -> Value {
    match v {
        Value::String(s) => match key {
            Some("Path") => Value::String(sanitize_agy_path(s, id_map)),
            Some("changedFiles") => Value::String(sanitize_agy_path(s, id_map)),
            Some("status") if s.len() <= 40 && !s.contains(char::is_whitespace) => {
                Value::String(s.clone())
            }
            _ => Value::String(scramble_text(s, cfg)),
        },
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|x| sanitize_agy_tool_args(key, x, id_map, cfg))
                .collect(),
        ),
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, x) in m {
                out.insert(k.clone(), sanitize_agy_tool_args(Some(k), x, id_map, cfg));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn sanitize_agy_step(v: &Value, id_map: &HashMap<String, String>, cfg: &Cfg) -> Value {
    let Some(m) = v.as_object() else {
        return v.clone();
    };
    let step_type = m.get("type").and_then(Value::as_str).unwrap_or("");
    let mut out = Map::new();
    for (k, x) in m {
        let sanitized = match (k.as_str(), x) {
            (key, Value::String(_)) if AGY_KEEP_STEP_KEYS.contains(&key) => x.clone(),
            ("content", Value::String(s)) => {
                let text = match step_type {
                    "USER_INPUT" => sanitize_agy_user_input(s, cfg),
                    "SYSTEM_MESSAGE" => sanitize_agy_system_message(s, id_map, cfg),
                    "GENERIC" => sanitize_agy_generic(s, id_map, cfg),
                    _ => scramble_text(s, cfg),
                };
                Value::String(text)
            }
            ("thinking" | "error", Value::String(s)) => Value::String(scramble_text(s, cfg)),
            ("tool_calls", Value::Array(calls)) => {
                let mapped: Vec<Value> = calls
                    .iter()
                    .map(|tc| {
                        let Some(obj) = tc.as_object() else {
                            return tc.clone();
                        };
                        let mut c = Map::new();
                        for (tk, tv) in obj {
                            let v2 = match (tk.as_str(), tv) {
                                ("name", Value::String(_)) => tv.clone(),
                                ("args", _) => sanitize_agy_tool_args(None, tv, id_map, cfg),
                                _ => sanitize_agy_tool_args(Some(tk), tv, id_map, cfg),
                            };
                            c.insert(tk.clone(), v2);
                        }
                        Value::Object(c)
                    })
                    .collect();
                Value::Array(mapped)
            }
            (_, Value::String(s)) => Value::String(scramble_text(s, cfg)),
            _ => x.clone(),
        };
        out.insert(k.clone(), sanitized);
    }
    Value::Object(out)
}

fn sanitize_agy_meta_entry(
    v: &Value,
    mapped_id: &str,
    id_map: &HashMap<String, String>,
    cfg: &Cfg,
) -> Value {
    let Some(m) = v.as_object() else {
        return v.clone();
    };
    let mut out = Map::new();
    for (k, x) in m {
        let v2 = match (k.as_str(), x) {
            ("summary", Value::Object(sm)) => {
                let mut sout = Map::new();
                for (sk, sx) in sm {
                    let sv = match (sk.as_str(), sx) {
                        ("ID", Value::String(_)) => Value::String(mapped_id.to_string()),
                        ("WorkspaceURIs", Value::Array(uris)) => Value::Array(
                            uris.iter()
                                .map(|u| match u.as_str() {
                                    Some(s) => Value::String(sanitize_agy_path(s, id_map)),
                                    None => u.clone(),
                                })
                                .collect(),
                        ),
                        ("CloudProject", Value::Object(cp)) => {
                            let mut cp_out = Map::new();
                            for (ck, cv) in cp {
                                let v = match cv.as_str() {
                                    Some("") => Value::String(String::new()),
                                    Some(s) => Value::String(scramble_text(s, cfg)),
                                    None => cv.clone(),
                                };
                                cp_out.insert(ck.clone(), v);
                            }
                            Value::Object(cp_out)
                        }
                        (key, Value::String(_)) if AGY_KEEP_META_KEYS.contains(&key) => sx.clone(),
                        (_, Value::String(s)) if s.is_empty() => Value::String(String::new()),
                        (_, Value::String(s)) => Value::String(scramble_text(s, cfg)),
                        _ => sx.clone(),
                    };
                    sout.insert(sk.clone(), sv);
                }
                Value::Object(sout)
            }
            (key, Value::String(_)) if AGY_KEEP_META_KEYS.contains(&key) => x.clone(),
            (_, Value::String(s)) => Value::String(scramble_text(s, cfg)),
            _ => x.clone(),
        };
        out.insert(k.clone(), v2);
    }
    Value::Object(out)
}

fn run_agy(mut args: impl Iterator<Item = String>) {
    let mut source: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut ids: Vec<String> = Vec::new();
    let mut lock_id: Option<String> = None;
    let mut cfg = Cfg {
        cwd: "/Users/dev/code/app".into(),
        cap: 160,
    };
    while let Some(a) = args.next() {
        let val = args.next().unwrap_or_default();
        match a.as_str() {
            "--source" => source = Some(PathBuf::from(val)),
            "--out" => out_dir = Some(PathBuf::from(val)),
            "--ids" => {
                ids = val
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "--lock" => lock_id = Some(val),
            "--cap" => cfg.cap = val.parse().expect("cap"),
            other => panic!("unknown flag {other}"),
        }
    }
    let source = source.expect("missing --source <agy_root>");
    let out_dir = out_dir.expect("missing --out <dir>");
    assert!(!ids.is_empty(), "missing --ids <id1,id2,...>");
    for id in &ids {
        assert!(
            transcript::agy::is_conversation_id(id),
            "not a conversation id: {id}"
        );
    }
    // Never write into the data root being read.
    // The output may not exist yet: resolve its nearest existing ancestor (symlinks such as
    // `/tmp` -> `/private/tmp` included) and append the rest.
    let resolved = |p: &std::path::Path| {
        let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let mut base = abs.as_path();
        let mut rest = Vec::new();
        while !base.exists() {
            let (Some(parent), Some(name)) = (base.parent(), base.file_name()) else {
                break;
            };
            rest.push(name.to_owned());
            base = parent;
        }
        let mut out = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
        out.extend(rest.into_iter().rev());
        out
    };
    assert!(
        !out_dir
            .components()
            .any(|c| c == std::path::Component::ParentDir),
        "--out must not contain .."
    );
    let out_dir = resolved(&out_dir);
    assert!(
        !out_dir.starts_with(resolved(&source)),
        "--out must be outside --source"
    );

    let id_map: HashMap<String, String> = ids
        .iter()
        .map(|orig| (orig.clone(), remap_uuid(orig)))
        .collect();

    // Sanitize each selected transcript (strictly read-only on source).
    for orig in &ids {
        let mapped = &id_map[orig];
        let src_path = source
            .join("brain")
            .join(orig)
            .join(".system_generated/logs/transcript_full.jsonl");
        let text = std::fs::read_to_string(&src_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", src_path.display()));
        let dst_path = out_dir
            .join("brain")
            .join(mapped)
            .join(".system_generated/logs/transcript_full.jsonl");
        std::fs::create_dir_all(dst_path.parent().unwrap()).expect("mkdir brain");
        let mut buf = String::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                eprintln!("{orig} line {}: not JSON, skipped", i + 1);
                continue;
            };
            buf.push_str(&sanitize_agy_step(&v, &id_map, &cfg).to_string());
            buf.push('\n');
        }
        std::fs::write(&dst_path, buf).expect("write transcript");
        eprintln!("{orig} -> {mapped}");
    }

    // Sanitize conversation_metadata.json (only entries for the selected ids).
    let meta_src = source.join("cache/conversation_metadata.json");
    let meta_raw: Value = serde_json::from_str(
        &std::fs::read_to_string(&meta_src)
            .unwrap_or_else(|e| panic!("read {}: {e}", meta_src.display())),
    )
    .expect("parse conversation_metadata.json");
    let src_convs = meta_raw
        .get("conversations")
        .and_then(Value::as_object)
        .expect("conversations object");
    let mut convs_out: BTreeMap<String, Value> = BTreeMap::new();
    for orig in &ids {
        let mapped = &id_map[orig];
        let entry = src_convs
            .get(orig)
            .unwrap_or_else(|| panic!("metadata missing entry for {orig}"));
        convs_out.insert(
            mapped.clone(),
            sanitize_agy_meta_entry(entry, mapped, &id_map, &cfg),
        );
    }
    let meta_dst = out_dir.join("cache/conversation_metadata.json");
    std::fs::create_dir_all(meta_dst.parent().unwrap()).expect("mkdir cache");
    let mut convs_map = Map::new();
    for (k, v) in convs_out {
        convs_map.insert(k, v);
    }
    let mut root_meta = Map::new();
    root_meta.insert("conversations".into(), Value::Object(convs_map));
    let meta_json =
        serde_json::to_string_pretty(&Value::Object(root_meta)).expect("format metadata");
    std::fs::write(&meta_dst, format!("{meta_json}\n")).expect("write metadata");

    // Optionally write one stale empty presence lock file (does not read real locks).
    if let Some(lock_orig) = lock_id {
        let mapped = id_map
            .get(&lock_orig)
            .unwrap_or_else(|| panic!("--lock id {lock_orig} must be in --ids"));
        let lock_dst = out_dir.join("presence").join(format!("{mapped}.lock"));
        std::fs::create_dir_all(lock_dst.parent().unwrap()).expect("mkdir presence");
        std::fs::write(&lock_dst, b"").expect("write lock");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().expect(
        "usage: sanitize <input> [--lines a-b,c] [--uuids a,b] [--uuid-file f] [--cwd p] [--cap n]\n       sanitize --agy --source <agy_root> --out <dir> --ids <id1,id2,...> [--lock <id>] [--cap n]",
    );
    if first == "--agy" {
        run_agy(args);
        return;
    }
    let input = first;
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
