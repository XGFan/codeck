//! Snippet construction (FTS `snippet()` markers or a ±60-character LIKE window) into `SnippetPart`s.
//!
//! Matching outside FTS is ASCII-case-insensitive only, like SQLite's `lower()` (A13).

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};

use crate::model::SnippetPart;

/// Highlight start / end markers passed to `snippet()` (private-use code points).
pub const HIT_START: char = '\u{E000}';
pub const HIT_END: char = '\u{E001}';
/// Characters of context on each side of the first hit in a LIKE-mode window.
pub const WINDOW_CHARS: usize = 60;
const ELLIPSIS: &str = "…";

/// Splits `snippet()` output on the hit markers.
pub fn parse_marked(s: &str) -> Vec<SnippetPart> {
    let mut parts = Vec::new();
    let mut hit = false;
    let mut cur = String::new();
    for c in s.chars() {
        let toggle = match c {
            HIT_START => Some(true),
            HIT_END => Some(false),
            _ => None,
        };
        match toggle {
            Some(next) => {
                push(&mut parts, std::mem::take(&mut cur), hit);
                hit = next;
            }
            None => cur.push(c),
        }
    }
    push(&mut parts, cur, hit);
    parts
}

fn push(parts: &mut Vec<SnippetPart>, text: String, hit: bool) {
    if text.is_empty() {
        return;
    }
    match parts.last_mut() {
        Some(last) if last.hit == hit => last.text.push_str(&text),
        _ => parts.push(SnippetPart { text, hit }),
    }
}

/// ASCII-case-insensitive matcher over `terms` (leftmost-longest, so overlapping terms merge).
pub fn matcher(terms: &[String]) -> Option<AhoCorasick> {
    let terms: Vec<&str> = terms
        .iter()
        .map(String::as_str)
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return None;
    }
    AhoCorasickBuilder::new()
        .ascii_case_insensitive(true)
        .match_kind(MatchKind::LeftmostLongest)
        .build(terms)
        .ok()
}

/// A window of [`WINDOW_CHARS`] characters around the first match of any term in `body`, with
/// every match inside it highlighted. Without a match, the start of `body`.
pub fn window(body: &str, terms: &[String]) -> Vec<SnippetPart> {
    let Some(ac) = matcher(terms) else {
        return clip(body, 0, body.len(), Vec::new());
    };
    let Some(first) = ac.find(body) else {
        let end = nth_char_end(body, 0, 2 * WINDOW_CHARS);
        return clip(body, 0, end, Vec::new());
    };
    let start = nth_char_start_back(body, first.start(), WINDOW_CHARS);
    let end = nth_char_end(body, first.end(), WINDOW_CHARS);
    let hits: Vec<(usize, usize)> = ac
        .find_iter(&body[start..end])
        .map(|m| (start + m.start(), start + m.end()))
        .collect();
    clip(body, start, end, hits)
}

/// Builds parts for `body[start..end]` with `hits` (absolute, sorted, non-overlapping) marked,
/// adding an ellipsis on each clipped side.
fn clip(body: &str, start: usize, end: usize, hits: Vec<(usize, usize)>) -> Vec<SnippetPart> {
    let mut parts = Vec::new();
    let mut lead = String::new();
    if start > 0 {
        lead.push_str(ELLIPSIS);
    }
    let mut pos = start;
    for (s, e) in hits {
        lead.push_str(&body[pos..s]);
        push(&mut parts, std::mem::take(&mut lead), false);
        push(&mut parts, body[s..e].to_owned(), true);
        pos = e;
    }
    lead.push_str(&body[pos..end]);
    if end < body.len() {
        lead.push_str(ELLIPSIS);
    }
    push(&mut parts, lead, false);
    parts
}

/// Byte offset `n` characters before `at` (or 0).
fn nth_char_start_back(s: &str, at: usize, n: usize) -> usize {
    s[..at]
        .char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map_or(0, |(i, _)| i)
}

/// Byte offset `n` characters after `at` (or the end).
fn nth_char_end(s: &str, at: usize, n: usize) -> usize {
    s[at..]
        .char_indices()
        .nth(n)
        .map_or(s.len(), |(i, _)| at + i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(parts: &[SnippetPart]) -> String {
        parts
            .iter()
            .map(|p| {
                if p.hit {
                    format!("[{}]", p.text)
                } else {
                    p.text.clone()
                }
            })
            .collect()
    }

    #[test]
    fn parses_snippet_markers() {
        let parts = parse_marked("…a \u{E000}Retry\u{E001} b \u{E000}x\u{E001}");
        assert_eq!(text(&parts), "…a [Retry] b [x]");
        assert_eq!(parts.len(), 4);
    }

    #[test]
    fn like_window_clips_and_highlights() {
        let body = format!("{}重试 and RETRY{}", "x".repeat(100), "y".repeat(100));
        let parts = window(&body, &["重试".to_owned(), "retry".to_owned()]);
        let t = text(&parts);
        assert!(t.starts_with('…') && t.ends_with('…'), "{t}");
        assert!(t.contains("[重试] and [RETRY]"), "{t}");
        assert_eq!(t.chars().filter(|&c| c == 'x').count(), WINDOW_CHARS);
        // No clipping when the body is short.
        assert_eq!(text(&window("ab cd", &["cd".to_owned()])), "ab [cd]");
    }
}
