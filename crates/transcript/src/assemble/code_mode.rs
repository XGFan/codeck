//! Claude Code's Code Mode (`CLAUDE_CODE_EXPERIMENTAL_CODE_MODE`; docs/spec.md「Code Mode」): the model
//! writes JavaScript for the `REPL` tool. The tool calls the code makes are recorded as separate
//! `isVirtual` messages after it, and what the code returned or threw arrives later as a `repl-eval`
//! event, in a `poll_events` attachment or a `Poll` result. Nothing links a virtual message to its
//! `REPL` call, so the owner is inferred from the order along the path.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::nodes::{RESULT_BYTES, RESULT_LINES, cap_text, image_info, result_content};
use super::{EntryKind, LoadedFile, SessionSkeleton};
use crate::model::{CodeOutcome, ImageRef};
use crate::raw::RawEntry;

/// The tool Claude's code runs in.
pub const REPL: &str = "REPL";
/// Waits for what running code sends back.
pub const POLL: &str = "Poll";

/// Code Mode's own tools, not counted as tool calls (the calls the code makes are).
pub fn is_plumbing(tool: &str) -> bool {
    tool == REPL || tool == POLL
}

/// One `repl-eval` event: the outcome of eval `eval` (the number `(dispatched #N)` names).
#[derive(Clone, Debug, PartialEq)]
pub struct EvalEvent {
    pub eval: u32,
    pub outcome: CodeOutcome,
    /// Its position among the events (of any kind) of the text it was parsed from.
    pub index: usize,
}

/// The `repl-eval` events in `text`: a `poll_events` envelope, a `Poll` result or a `REPL` result
/// (`<repl-output eval="N">`, when the code finished before the call returned). A delivery that
/// lists its event nonces (`authentic event nonces for this delivery: …`) counts only events
/// carrying one of them; any other `<event>` is quoted text.
pub fn parse_events(text: &str) -> Vec<EvalEvent> {
    let nonces = delivery_nonces(text);
    let mut out = Vec::new();
    let mut index = 0;
    let mut rest = text;
    while let Some(start) = next_open(rest) {
        let (tag, close) = if rest[start..].starts_with("<event") {
            ("event", "</event>")
        } else {
            ("repl-output", "</repl-output>")
        };
        let after = &rest[start + 1 + tag.len()..];
        let Some(head_end) = after.find('>') else {
            break;
        };
        let attrs = attributes(&after[..head_end]);
        let body_and_rest = &after[head_end + 1..];
        let Some(body_end) = body_and_rest.find(close) else {
            break;
        };
        let body = &body_and_rest[..body_end];
        rest = &body_and_rest[body_end + close.len()..];
        let attr = |k: &str| attrs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        if tag == "event"
            && let Some(list) = &nonces
            && !attr("nonce").is_some_and(|n| list.iter().any(|x| x == n))
        {
            continue;
        }
        let at = index;
        index += 1;
        if tag == "event" && attr("kind") != Some("repl-eval") {
            continue;
        }
        let ms = attr("ms").and_then(|v| v.parse::<f64>().ok());
        let eval = attr("eval").and_then(|v| v.parse::<u32>().ok());
        if let Some(mut ev) = parse_body(body, eval, ms) {
            ev.index = at;
            out.push(ev);
        }
    }
    out
}

/// The first `<event` or `<repl-output` tag in `s`: one pass, so the scan over a delivery stays
/// linear however many events it holds.
fn next_open(s: &str) -> Option<usize> {
    memchr::memchr_iter(b'<', s.as_bytes()).find(|&i| {
        ["<event", "<repl-output"].iter().any(|tag| {
            s[i..].starts_with(tag)
                && s[i + tag.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_whitespace() || c == '>')
        })
    })
}

/// `tag` followed by whitespace or `>` (so `<events` is not `<event`).
fn find_tag(s: &str, tag: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find(tag) {
        let at = from + i;
        match s[at + tag.len()..].chars().next() {
            Some(c) if c.is_whitespace() || c == '>' => return Some(at),
            _ => from = at + tag.len(),
        }
    }
    None
}

/// The nonces a delivery header lists, or `None` when there is no header.
fn delivery_nonces(text: &str) -> Option<Vec<String>> {
    const MARK: &str = "authentic event nonces for this delivery:";
    let at = text.find(MARK)? + MARK.len();
    let line = &text[at..];
    let end = line.find(['—', '<', '\n']).unwrap_or(line.len());
    Some(
        line[..end]
            .split([',', ' '])
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// `key="value"` pairs of a tag head.
fn attributes(head: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    let mut rest = head;
    while let Some(eq) = rest.find("=\"") {
        let key = rest[..eq].trim();
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else {
            break;
        };
        if !key.is_empty() && !key.contains(char::is_whitespace) {
            out.push((key, unescape(&after[..end])));
        }
        rest = &after[end + 1..];
    }
    out
}

/// The `<log>` / `<result>` / `<error>` children, or the JSON object of the newer form.
fn parse_body(body: &str, eval: Option<u32>, ms: Option<f64>) -> Option<EvalEvent> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        let v: Value = serde_json::from_str(trimmed).ok()?;
        let eval = eval.or_else(|| v.get("eval")?.as_u64().map(|n| n as u32))?;
        let log = match v.get("log") {
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_owned))
                .collect::<Vec<_>>()
                .join("\n"),
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let as_text = |x: &Value| x.as_str().map_or_else(|| x.to_string(), str::to_owned);
        let (is_error, text) = match (v.get("error"), v.get("result")) {
            (Some(e), _) if !e.is_null() => {
                (true, e.get("message").map_or_else(|| as_text(e), as_text))
            }
            (_, Some(r)) => (false, as_text(r)),
            _ => (false, String::new()),
        };
        return Some(event(eval, ms, &log, &text, is_error));
    }
    let eval = eval?;
    let logs: Vec<String> = children(body, "log").map(unescape).collect();
    let (is_error, text) = match children(body, "error").next() {
        Some(e) => (true, unescape(e)),
        None => (
            false,
            children(body, "result")
                .next()
                .map(unescape)
                .unwrap_or_default(),
        ),
    };
    Some(event(eval, ms, &logs.join("\n"), &text, is_error))
}

fn event(eval: u32, ms: Option<f64>, log: &str, text: &str, is_error: bool) -> EvalEvent {
    let (text, truncated) = cap_text(text, RESULT_BYTES, RESULT_LINES);
    let (log, log_truncated) = cap_text(log, RESULT_BYTES, RESULT_LINES);
    EvalEvent {
        eval,
        outcome: CodeOutcome {
            is_error,
            text,
            log,
            truncated: truncated || log_truncated,
            duration_ms: ms,
        },
        index: 0,
    }
}

/// Contents of each `<tag>…</tag>` in `body` (no nesting: contents are XML-escaped).
fn children<'a>(body: &'a str, tag: &str) -> impl Iterator<Item = &'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut rest = body;
    std::iter::from_fn(move || {
        let s = rest.find(&open)? + open.len();
        let e = rest[s..].find(&close)? + s;
        let item = &rest[s..e];
        rest = &rest[e + close.len()..];
        Some(item)
    })
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Code runs along a path: which virtual entries belong to which `REPL` call, and their outcomes.
#[derive(Debug, Default)]
pub struct CodeRuns {
    /// Virtual assistant entry (index into the skeleton) → the `REPL` tool_use id it belongs to.
    pub owner: HashMap<usize, String>,
    /// `REPL` tool_use id → its virtual assistant entries, in path order.
    pub calls: HashMap<String, Vec<usize>>,
    /// `REPL` tool_use id → what the code returned or threw (the first delivery wins).
    pub outcome: HashMap<String, CodeOutcome>,
    /// `REPL` tool_use id → the pictures delivered with its outcome (the files its code read as
    /// pictures, in the order it read them), each addressed where it is stored: the `Poll` or
    /// `REPL` result, or the `poll_events` attachment.
    pub media: HashMap<String, Vec<ImageRef>>,
}

#[derive(Deserialize)]
struct PollEvents<'a> {
    #[serde(rename = "type", default)]
    kind: Option<&'a str>,
    #[serde(default)]
    envelopes: Vec<String>,
    /// What each envelope's event showed Claude besides text.
    #[serde(default, borrow)]
    media: Vec<Vec<MediaItem<'a>>>,
}

#[derive(Deserialize)]
struct MediaItem<'a> {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default, borrow)]
    source: Option<&'a RawValue>,
}

/// `toolUseResult` of a `Poll` result: how many media items each event of the delivery brought.
#[derive(Deserialize)]
struct PollMedia {
    #[serde(default)]
    media: Vec<Vec<serde::de::IgnoredAny>>,
}

/// The `source` of each picture in a `poll_events` attachment's `media`, with the envelope it came
/// with; an [`ImageRef`] into the attachment counts them in this order.
fn attachment_pictures<'a>(p: &PollEvents<'a>) -> Vec<(usize, &'a RawValue)> {
    p.media
        .iter()
        .enumerate()
        .flat_map(|(k, items)| items.iter().map(move |it| (k, it)))
        .filter(|(_, it)| it.kind.as_deref() == Some("image"))
        .filter_map(|(k, it)| Some((k, it.source?)))
        .collect()
}

/// The `source` of picture `ordinal` of a `poll_events` attachment.
pub fn attachment_picture(raw: Option<&RawValue>, ordinal: usize) -> Option<&RawValue> {
    let p: PollEvents<'_> = serde_json::from_str(raw?.get()).ok()?;
    attachment_pictures(&p).get(ordinal).map(|&(_, s)| s)
}

/// Whether `toolUseResult` is that of a file read as a picture (`{"type":"image", …}`): the picture
/// itself is not there, it was shown to Claude with the code's outcome.
pub fn is_picture_result(raw: Option<&RawValue>) -> bool {
    #[derive(Deserialize)]
    struct Kind<'a> {
        #[serde(rename = "type", default)]
        kind: Option<&'a str>,
    }
    raw.map(RawValue::get)
        .filter(|s| memchr::memmem::find(s.as_bytes(), b"\"image\"").is_some())
        .and_then(|s| serde_json::from_str::<Kind<'_>>(s).ok())
        .is_some_and(|k| k.kind == Some("image"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplResult {
    #[serde(default)]
    eval_id: Option<u32>,
}

/// Walks `order` (entries in path order, root first; satellites may be mixed in): a `REPL` call makes
/// it the current one, an outcome delivered for a known eval makes that eval's call current, and a
/// virtual assistant entry belongs to the current call. A delivery with several outcomes leaves the
/// last one's call current. Virtual entries before any `REPL` call belong to none.
pub fn collect(sk: &SessionSkeleton, files: &[LoadedFile], order: &[usize]) -> CodeRuns {
    let mut runs = CodeRuns::default();
    if !order
        .iter()
        .any(|&i| sk.nodes[i].tool_uses.iter().any(|(_, n)| n == REPL))
    {
        return runs;
    }
    let tool_name = |id: &str| -> Option<&str> {
        let &at = sk.tool_use_at.get(id)?;
        sk.nodes[at]
            .tool_uses
            .iter()
            .find(|(t, _)| t == id)
            .map(|(_, n)| n.as_str())
    };
    let mut in_order = vec![false; sk.nodes.len()];
    for &i in order {
        in_order[i] = true;
    }
    // eval number → the latest `REPL` call (so far along the path) that was given it.
    let mut eval_owner: HashMap<u32, String> = HashMap::new();
    let mut current: Option<String> = None;
    for &i in order {
        let n = &sk.nodes[i];
        match n.entry_type {
            EntryKind::Assistant if n.is_virtual => {
                if let Some(c) = &current {
                    runs.owner.insert(i, c.clone());
                    runs.calls.entry(c.clone()).or_default().push(i);
                }
            }
            EntryKind::Assistant => {
                // A `REPL` call's result can hang off the call, beside the `Poll` made with it, whose
                // result (on the path) carries the outcome: read it here so its eval number is known.
                for (id, _) in n.tool_uses.iter().filter(|(_, t)| t == REPL) {
                    let rs = sk.results_for.get(id).map(Vec::as_slice).unwrap_or(&[]);
                    if !rs.iter().any(|&r| in_order[r])
                        && let Some(&r) = rs.first()
                    {
                        let e = sk.entry(files, r);
                        take_result(&mut runs, &mut current, &mut eval_owner, e, id, REPL);
                    }
                }
                if let Some((id, _)) = n.tool_uses.iter().rev().find(|(_, t)| t == REPL) {
                    current = Some(id.clone());
                }
            }
            EntryKind::User if n.is_tool_result && !n.is_virtual => {
                let e = sk.entry(files, i);
                for id in &n.tool_results {
                    let Some(name) = tool_name(id) else { continue };
                    if name == REPL || name == POLL {
                        take_result(&mut runs, &mut current, &mut eval_owner, e, id, name);
                    }
                }
            }
            EntryKind::Attachment => {
                let Some(raw) = sk.entry(files, i).attachment.as_deref() else {
                    continue;
                };
                if memchr::memmem::find(raw.get().as_bytes(), b"poll_events").is_none() {
                    continue;
                }
                let Ok(p) = serde_json::from_str::<PollEvents<'_>>(raw.get()) else {
                    continue;
                };
                if p.kind != Some("poll_events") {
                    continue;
                }
                // One event to an envelope: an event's media is its envelope's.
                let events: Vec<EvalEvent> = p
                    .envelopes
                    .iter()
                    .enumerate()
                    .flat_map(|(k, s)| {
                        parse_events(s).into_iter().map(move |mut ev| {
                            ev.index = k;
                            ev
                        })
                    })
                    .collect();
                let mut media: Vec<Vec<ImageRef>> = Vec::new();
                for (ordinal, (k, source)) in attachment_pictures(&p).into_iter().enumerate() {
                    let (media_type, bytes) = image_info(Some(source));
                    media.resize_with(media.len().max(k + 1), Vec::new);
                    media[k].push(ImageRef {
                        node_id: n.uuid.clone(),
                        ordinal: ordinal as u32,
                        media_type,
                        bytes,
                        tool_use_id: None,
                    });
                }
                deliver(&mut runs, &mut current, &eval_owner, events, media);
            }
            _ => {}
        }
    }
    runs
}

/// A `REPL` or `Poll` result (tool_use `id` in entry `e`): a `REPL` result names its eval number;
/// either may deliver outcomes.
fn take_result(
    runs: &mut CodeRuns,
    current: &mut Option<String>,
    eval_owner: &mut HashMap<u32, String>,
    e: &RawEntry,
    id: &str,
    name: &str,
) {
    let Some(block) = e
        .blocks()
        .iter()
        .find(|b| b.tool_use_id.as_deref() == Some(id))
    else {
        return;
    };
    let (text, images) = result_content(block.content.as_deref());
    if name == REPL
        && let Some(n) = eval_number(e, &text)
    {
        eval_owner.insert(n, id.to_owned());
    }
    let events = parse_events(&text);
    let media = result_media(e, id, images, &events);
    deliver(runs, current, eval_owner, events, media);
}

/// The pictures of a `Poll` or `REPL` result (tool_use `id` in entry `e`) by the event they came
/// with: `toolUseResult.media` says how many each event brought; without it they are the last
/// outcome's (a delivery with pictures has had one outcome so far).
fn result_media(
    e: &RawEntry,
    id: &str,
    images: Vec<(String, f64)>,
    events: &[EvalEvent],
) -> Vec<Vec<ImageRef>> {
    if images.is_empty() {
        return Vec::new();
    }
    let total = images.len();
    let mut refs = images
        .into_iter()
        .enumerate()
        .map(|(k, (media_type, bytes))| ImageRef {
            node_id: e.uuid.clone().unwrap_or_default(),
            ordinal: k as u32,
            media_type,
            bytes,
            tool_use_id: Some(id.to_owned()),
        });
    let counts = e
        .tool_use_result
        .as_deref()
        .filter(|raw| raw.get().starts_with('{'))
        .and_then(|raw| serde_json::from_str::<PollMedia>(raw.get()).ok())
        .map(|m| m.media.iter().map(Vec::len).collect::<Vec<_>>())
        .filter(|c| c.iter().sum::<usize>() == total);
    match counts {
        Some(counts) => counts
            .into_iter()
            .map(|n| refs.by_ref().take(n).collect())
            .collect(),
        None => {
            let mut out = Vec::new();
            if let Some(last) = events.last() {
                out.resize_with(last.index + 1, Vec::new);
                out[last.index] = refs.collect();
            }
            out
        }
    }
}

/// `media[k]`: the pictures that came with the delivery's event `k`.
fn deliver(
    runs: &mut CodeRuns,
    current: &mut Option<String>,
    eval_owner: &HashMap<u32, String>,
    events: Vec<EvalEvent>,
    mut media: Vec<Vec<ImageRef>>,
) {
    for ev in events {
        let Some(id) = eval_owner.get(&ev.eval) else {
            continue;
        };
        if !runs.outcome.contains_key(id) {
            runs.outcome.insert(id.clone(), ev.outcome);
            if let Some(m) = media.get_mut(ev.index).filter(|m| !m.is_empty()) {
                runs.media.insert(id.clone(), std::mem::take(m));
            }
        }
        *current = Some(id.clone());
    }
}

/// The eval number of a `REPL` result: `toolUseResult.evalId`, else `(dispatched #N)` or
/// `<repl-output eval="N">` in its text.
fn eval_number(e: &RawEntry, text: &str) -> Option<u32> {
    if let Some(raw) = e.tool_use_result.as_deref()
        && raw.get().starts_with('{')
        && let Ok(r) = serde_json::from_str::<ReplResult>(raw.get())
        && let Some(n) = r.eval_id
    {
        return Some(n);
    }
    let t = text.trim();
    if let Some(n) = t
        .strip_prefix("(dispatched #")
        .and_then(|r| r.strip_suffix(')'))
        .and_then(|n| n.parse().ok())
    {
        return Some(n);
    }
    let head = &t[find_tag(t, "<repl-output")?..];
    let head = &head[..head.find('>')?];
    attributes(head)
        .into_iter()
        .find(|(k, _)| *k == "eval")
        .and_then(|(_, v)| v.parse().ok())
}

/// Output text of a call Code Mode's code made: its tool_result content is usually empty and the data
/// is in `toolUseResult` — a file read (`cat -n` style, as a Read result), a command's stdout / stderr,
/// a `content` string or a list of file names. `None` for other shapes.
pub fn virtual_output(raw: Option<&RawValue>) -> Option<String> {
    let v: Value = serde_json::from_str(raw?.get()).ok()?;
    if let Some(s) = v.as_str() {
        return Some(s.to_owned());
    }
    if let Some(file) = v.get("file")
        && let Some(content) = file.get("content").and_then(Value::as_str)
    {
        let start = file.get("startLine").and_then(Value::as_u64).unwrap_or(1);
        let body = content.strip_suffix('\n').unwrap_or(content);
        if body.is_empty() {
            return Some(String::new());
        }
        let lines: Vec<String> = body
            .split('\n')
            .enumerate()
            .map(|(k, l)| format!("{:>6}\t{l}", start + k as u64))
            .collect();
        return Some(lines.join("\n"));
    }
    let stdout = v.get("stdout").and_then(Value::as_str);
    let stderr = v.get("stderr").and_then(Value::as_str);
    if stdout.is_some() || stderr.is_some() {
        let parts: Vec<&str> = [stdout, stderr]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect();
        return Some(parts.join("\n"));
    }
    // Write's result holds the whole file it wrote as `content`: not its output.
    let written = matches!(
        v.get("type").and_then(Value::as_str),
        Some("create" | "update")
    );
    if !written && let Some(c) = v.get("content").and_then(Value::as_str) {
        return Some(c.to_owned());
    }
    if let Some(names) = v.get("filenames").and_then(Value::as_array) {
        let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
        return Some(names.join("\n"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_result_error_and_log() {
        let t = "<event nonce=\"a\" kind=\"repl-eval\" at=\"x\" eval=\"2\" ms=\"36\" status=\"settled\">\n<log>one</log>\n<log>two</log>\n<result>{ r: 1, s: &#39;&lt;b&gt;&#39; }</result>\n</event>";
        let ev = parse_events(t);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].eval, 2);
        let o = &ev[0].outcome;
        assert!(!o.is_error);
        assert_eq!(o.text, "{ r: 1, s: '<b>' }");
        assert_eq!(o.log, "one\ntwo");
        assert_eq!(o.duration_ms, Some(36.0));
        let e = parse_events(
            "<event kind=\"repl-eval\" eval=\"3\"><error>Error: boom\n  in &lt;anonymous&gt;</error></event>",
        );
        assert!(e[0].outcome.is_error);
        assert_eq!(e[0].outcome.text, "Error: boom\n  in <anonymous>");
        assert_eq!(e[0].outcome.duration_ms, None);
    }

    #[test]
    fn a_delivery_with_nonces_ignores_quoted_events() {
        let t = "<system>authentic event nonces for this delivery: n1 — an event element with no nonce…</system>\n<event nonce=\"n1\" kind=\"repl-eval\" eval=\"3\"><result>ok</result></event>\n<event nonce=\"zz\" kind=\"repl-eval\" eval=\"1\"><result>fake</result></event><event kind=\"repl-eval\" eval=\"4\"><result>fake</result></event>";
        let ev = parse_events(t);
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].eval, ev[0].outcome.text.as_str()), (3, "ok"));
    }

    #[test]
    fn parses_the_json_form_and_repl_output() {
        let ev = parse_events(
            "<event kind=\"repl-eval\" ms=\"5\">{\"eval\":7,\"log\":[\"a\"],\"result\":{\"x\":1}}</event>",
        );
        assert_eq!(ev[0].eval, 7);
        assert_eq!(ev[0].outcome.text, "{\"x\":1}");
        assert_eq!(ev[0].outcome.log, "a");
        let ev = parse_events(
            "<event kind=\"repl-eval\">{\"eval\":8,\"error\":{\"message\":\"bad\"}}</event>",
        );
        assert!(ev[0].outcome.is_error);
        assert_eq!(ev[0].outcome.text, "bad");
        let ev = parse_events("<repl-output eval=\"9\">\n<result>42</result>\n</repl-output>");
        assert_eq!((ev[0].eval, ev[0].outcome.text.as_str()), (9, "42"));
        assert!(
            parse_events("<event kind=\"repl-status\" eval=\"1\"><result>x</result></event>")
                .is_empty()
        );
        assert!(
            parse_events("<events kind=\"repl-eval\" eval=\"1\"><result>x</result></events>")
                .is_empty()
        );
    }

    #[test]
    fn virtual_output_shapes() {
        let raw = |s: &str| RawValue::from_string(s.to_owned()).unwrap();
        assert_eq!(
            virtual_output(Some(&raw(
                r#"{"type":"text","file":{"content":"a\nb\n","startLine":5}}"#
            )))
            .as_deref(),
            Some("     5\ta\n     6\tb")
        );
        assert_eq!(
            virtual_output(Some(&raw(
                r#"{"stdout":"out","stderr":"","interrupted":false}"#
            )))
            .as_deref(),
            Some("out")
        );
        assert_eq!(
            virtual_output(Some(&raw(r#"{"stdout":"out","stderr":"err"}"#))).as_deref(),
            Some("out\nerr")
        );
        assert_eq!(
            virtual_output(Some(&raw(r#"{"filenames":["a.rs","b.rs"],"numFiles":2}"#))).as_deref(),
            Some("a.rs\nb.rs")
        );
        assert_eq!(
            virtual_output(Some(&raw(r#"{"structuredPatch":[]}"#))),
            None
        );
        assert_eq!(
            virtual_output(Some(&raw(
                r#"{"type":"create","filePath":"a","content":"file"}"#
            ))),
            None
        );
        assert_eq!(
            virtual_output(Some(&raw(r#"{"content":"matches","mode":"content"}"#))).as_deref(),
            Some("matches")
        );
        assert_eq!(virtual_output(None), None);
    }
}
