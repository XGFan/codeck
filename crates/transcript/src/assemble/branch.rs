//! Branch points, the hidden-resend rule and paths from `branch_choices` (§3.4 step 7, A14d).

use super::{LoadedFile, SessionSkeleton, prompt, tree};
use crate::model::{BranchChoice, BranchOption, BranchPoint};
use crate::raw::decode_str;

/// Preview length of a branch option in characters.
const PREVIEW_CHARS: usize = 100;

/// Resolves `anchor_key` (`"root"` or an anchor uuid) to an anchor; `None` when unknown.
fn parse_anchor(s: &SessionSkeleton, key: &str) -> Option<Option<usize>> {
    if key == "root" {
        Some(None)
    } else {
        s.by_uuid.get(key).map(|&i| Some(i))
    }
}

/// The selected path: the Main Line, with each matching choice switching to its head (root ..
/// head, then the newest leaf below it). Choices further down apply to the new path. Unknown or
/// stale choices are ignored.
pub fn resolve_path(s: &SessionSkeleton, choices: &[BranchChoice]) -> Vec<usize> {
    let mut path = s.main_path.clone();
    if choices.is_empty() {
        return path;
    }
    let resolved: Vec<Option<(Option<usize>, usize)>> = choices
        .iter()
        .map(|c| {
            let anchor = parse_anchor(s, &c.anchor_key)?;
            let head = *s.by_uuid.get(&c.head_id)?;
            (s.nodes[head].is_head && s.anchor[head] == anchor).then_some((anchor, head))
        })
        .collect();
    let mut used = vec![false; choices.len()];
    let mut i = 0;
    while i < path.len() {
        let v = path[i];
        if s.nodes[v].is_head {
            let hit = resolved.iter().enumerate().find_map(|(ci, r)| match r {
                Some((a, h)) if !used[ci] && *a == s.anchor[v] => Some((ci, *h)),
                _ => None,
            });
            if let Some((ci, head)) = hit {
                used[ci] = true;
                if head != v {
                    path = tree::path_through(s, head);
                    i = path.iter().position(|&x| x == head).unwrap_or(i);
                }
            }
        }
        i += 1;
    }
    path
}

/// BranchPoints for the heads on `path` whose anchor group has ≥ 2 heads, after the
/// hidden-resend rule (never dropping the selected head).
pub fn branch_points(
    s: &SessionSkeleton,
    files: &[LoadedFile],
    path: &[usize],
) -> Vec<BranchPoint> {
    let mut out = Vec::new();
    for &v in path {
        if !s.nodes[v].is_head {
            continue;
        }
        let Some(group) = s.heads_by_anchor.get(&s.anchor[v]) else {
            continue;
        };
        if group.len() < 2 {
            continue;
        }
        let selected_text = s.nodes[v]
            .is_human
            .then(|| prompt::user_text(s.entry(files, v)));
        let mut options = Vec::with_capacity(group.len());
        for &h in group {
            let node = &s.nodes[h];
            let text = if node.is_human {
                prompt::user_text(s.entry(files, h))
            } else {
                assistant_preview(s, files, h)
            };
            if h != v
                && node.is_human
                && s.subtree_assistants[h] == 0
                && let Some(sel) = &selected_text
            {
                let (t, sel) = (text.trim(), sel.trim());
                if t == sel || sel.starts_with(t) {
                    continue;
                }
            }
            let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
            options.push(BranchOption {
                head_id: node.uuid.clone(),
                preview: prompt::truncate_chars(&collapsed, PREVIEW_CHARS),
                timestamp_ms: node.timestamp_ms,
                reply_count: s.subtree_assistants[h] - u32::from(node.is_assistant()),
                is_main_line: s.on_main.get(h).copied().unwrap_or(false),
            });
        }
        if options.len() >= 2 {
            out.push(BranchPoint {
                anchor_key: s.anchor_key(v),
                selected_head_id: s.nodes[v].uuid.clone(),
                options,
            });
        }
    }
    out
}

/// First non-empty text block of an assistant message (all fragments), else its first tool name.
fn assistant_preview(s: &SessionSkeleton, files: &[LoadedFile], head: usize) -> String {
    let frags: &[usize] = match s.nodes[head]
        .message_id
        .as_ref()
        .and_then(|m| s.fragments.get(m))
    {
        Some(f) => f,
        None => std::slice::from_ref(&head),
    };
    for &f in frags {
        for b in s.entry(files, f).blocks() {
            if b.block_type() == "text"
                && let Some(t) = decode_str(b.text.as_deref()).filter(|t| !t.trim().is_empty())
            {
                return t;
            }
        }
    }
    frags
        .iter()
        .find_map(|&f| s.nodes[f].tool_uses.first().map(|(_, name)| name.clone()))
        .unwrap_or_default()
}
