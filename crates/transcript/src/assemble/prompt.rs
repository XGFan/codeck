//! `PromptOrigin` classification; `<command-name>`, `<task-notification>` and
//! `<teammate-message>` parsing.

use crate::model::PromptOrigin;
use crate::raw::RawEntry;

/// Line Claude Code puts before a `<teammate-message>` it relays to the lead session.
const TEAMMATE_PREAMBLE: &str = "Another Claude session sent a message:";

/// Title / first-prompt length cap in characters (§3.4 step 9).
pub const TITLE_CHARS: usize = 120;

/// Text of a user message: the plain string content, or its `text` blocks joined by `\n`.
pub fn user_text(e: &RawEntry) -> String {
    if let Some(t) = e.content_text() {
        return t.to_owned();
    }
    let mut out = String::new();
    for b in e.blocks() {
        if b.block_type() == "text"
            && let Some(t) = b.text_str()
        {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&t);
        }
    }
    out
}

/// Classifies a user entry that is neither a tool result nor a compact summary.
pub fn classify(e: &RawEntry, text: &str) -> PromptOrigin {
    if e.is_meta == Some(true) {
        return PromptOrigin::Meta;
    }
    let t = text.trim_start();
    let origin_kind = e.origin.as_ref().and_then(|o| o.kind.as_deref());
    if origin_kind == Some("task-notification") || t.starts_with("<task-notification>") {
        return PromptOrigin::TaskNotification {
            task_id: tag(t, "task-id"),
            tool_use_id: tag(t, "tool-use-id"),
            status: tag(t, "status"),
            summary: tag(t, "summary"),
        };
    }
    let teammate = t.strip_prefix(TEAMMATE_PREAMBLE).map_or(t, str::trim_start);
    if teammate.starts_with("<teammate-message") {
        return PromptOrigin::Teammate {
            teammate_id: attr(teammate, "teammate_id"),
            color: attr(teammate, "color"),
            summary: attr(teammate, "summary"),
        };
    }
    if (t.starts_with("<command-name>") || t.starts_with("<command-message>"))
        && let Some(name) = tag(t, "command-name")
    {
        // Claude Code stores the name as typed ("/release"); a few entries omit the slash.
        return PromptOrigin::Command {
            name: format!("/{}", name.trim_start_matches('/')),
            args: tag(t, "command-args").unwrap_or_default(),
        };
    }
    if t.starts_with("<local-command-stdout>")
        || t.starts_with("<local-command-stderr>")
        || t.starts_with("<bash-stdout>")
        || t.starts_with("<bash-stderr>")
    {
        return PromptOrigin::CommandOutput;
    }
    if t.starts_with("<local-command-caveat>") || t.starts_with("<system-reminder>") {
        return PromptOrigin::Meta;
    }
    PromptOrigin::Human
}

/// Tags wrapping a command's output in a [`PromptOrigin::CommandOutput`] prompt.
const OUTPUT_TAGS: [&str; 4] = [
    "local-command-stdout",
    "local-command-stderr",
    "bash-stdout",
    "bash-stderr",
];

/// A command output with nothing inside its tags (`<local-command-stdout></local-command-stdout>`).
pub fn is_empty_output(text: &str) -> bool {
    let mut rest = text.to_owned();
    for t in OUTPUT_TAGS {
        rest = rest
            .replace(&format!("<{t}>"), "")
            .replace(&format!("</{t}>"), "");
    }
    rest.trim().is_empty()
}

/// A human or command prompt typed by the user: counts as a message, can head a Branch and
/// supplies the title. Excludes other origins (`peer`, …), meta, outputs and notifications.
pub fn is_human(e: &RawEntry, origin: &PromptOrigin) -> bool {
    let kind_ok = match e.origin.as_ref().and_then(|o| o.kind.as_deref()) {
        None | Some("human") => true,
        Some(_) => false,
    };
    kind_ok && matches!(origin, PromptOrigin::Human | PromptOrigin::Command { .. })
}

/// [`is_human`] for any entry: a `user` entry that is not a tool result or compact summary and
/// classifies as a human or command prompt. Shared by assembly, the index and text extraction.
pub fn is_human_prompt(e: &RawEntry) -> bool {
    if e.entry_type() != "user" || e.is_tool_result() || e.is_compact_summary == Some(true) {
        return false;
    }
    let text = user_text(e);
    is_human(e, &classify(e, &text))
}

/// Title form of a prompt: `"name args"` for a command, else the text; whitespace collapsed,
/// truncated to [`TITLE_CHARS`] characters.
pub fn title_text(origin: &PromptOrigin, text: &str) -> String {
    let raw = match origin {
        PromptOrigin::Command { name, args } => format!("{name} {args}"),
        _ => text.to_owned(),
    };
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, TITLE_CHARS)
}

/// First `n` characters of `s`.
pub fn truncate_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => s[..i].to_owned(),
        None => s.to_owned(),
    }
}

/// Value of `name="…"` in the opening tag at the start of `s`; `None` when absent or empty.
fn attr(s: &str, name: &str) -> Option<String> {
    let open = &s[..s.find('>')?];
    let key = format!(" {name}=\"");
    let start = open.find(&key)? + key.len();
    let end = open[start..].find('"')? + start;
    let v = open[start..end].trim();
    (!v.is_empty()).then(|| v.to_owned())
}

/// Inner text of the first `<name>…</name>` in `s`, trimmed; `None` when absent or empty.
pub fn tag(s: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close)? + start;
    let v = s[start..end].trim();
    (!v.is_empty()).then(|| v.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(json: &str) -> RawEntry {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn classifies_commands_notifications_and_outputs() {
        let e = entry(r#"{"type":"user","message":{"content":"x"}}"#);
        let cmd = "<command-message>release</command-message>\n<command-name>/release</command-name>\n<command-args>v1.2.0</command-args>";
        let o = classify(&e, cmd);
        assert_eq!(
            o,
            PromptOrigin::Command {
                name: "/release".into(),
                args: "v1.2.0".into()
            }
        );
        assert_eq!(title_text(&o, cmd), "/release v1.2.0");
        let bare = "<command-message>codex-cli-runtime</command-message>\n<command-name>codex-cli-runtime</command-name>";
        assert!(
            matches!(classify(&e, bare), PromptOrigin::Command { name, .. } if name == "/codex-cli-runtime")
        );
        assert!(is_human(&e, &o));

        let n = "<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n<summary>done</summary>\n</task-notification>";
        match classify(&e, n) {
            PromptOrigin::TaskNotification {
                task_id,
                tool_use_id,
                status,
                summary,
            } => {
                assert_eq!(task_id.as_deref(), Some("a1"));
                assert_eq!(tool_use_id.as_deref(), Some("toolu_1"));
                assert_eq!(status.as_deref(), Some("completed"));
                assert_eq!(summary.as_deref(), Some("done"));
            }
            o => panic!("unexpected {o:?}"),
        }
        assert_eq!(
            classify(&e, "<local-command-stdout>ok</local-command-stdout>"),
            PromptOrigin::CommandOutput
        );
        assert!(is_empty_output(
            "<local-command-stdout> </local-command-stdout>"
        ));
        assert!(!is_empty_output(
            "<local-command-stdout>ok</local-command-stdout>"
        ));
        let meta = entry(r#"{"type":"user","isMeta":true}"#);
        assert_eq!(classify(&meta, "hi"), PromptOrigin::Meta);
        let peer = entry(r#"{"type":"user","origin":{"kind":"peer"}}"#);
        assert!(!is_human(&peer, &classify(&peer, "hi")));
    }

    #[test]
    fn classifies_teammate_messages() {
        let e = entry(r#"{"type":"user","message":{"content":"x"}}"#);
        let relayed = "Another Claude session sent a message:\n<teammate-message teammate_id=\"gui-impl\" color=\"red\" summary=\"GUI done\">\nbody\n</teammate-message>\n\nThis came from another Claude session";
        let o = classify(&e, relayed);
        assert_eq!(
            o,
            PromptOrigin::Teammate {
                teammate_id: Some("gui-impl".into()),
                color: Some("red".into()),
                summary: Some("GUI done".into()),
            }
        );
        assert!(!is_human(&e, &o));
        let bare = "<teammate-message teammate_id=\"app\">\n{\"type\":\"idle_notification\"}\n</teammate-message>";
        assert_eq!(
            classify(&e, bare),
            PromptOrigin::Teammate {
                teammate_id: Some("app".into()),
                color: None,
                summary: None,
            }
        );
        // A typed prompt that merely mentions the tag stays human.
        let typed = "Fix how `<teammate-message teammate_id=\"x\">` renders";
        assert_eq!(classify(&e, typed), PromptOrigin::Human);
    }

    #[test]
    fn title_collapses_whitespace_and_truncates() {
        let t = title_text(
            &PromptOrigin::Human,
            &format!("  a\n\nb {}", "x".repeat(200)),
        );
        assert!(t.starts_with("a b x"));
        assert_eq!(t.chars().count(), TITLE_CHARS);
    }
}
