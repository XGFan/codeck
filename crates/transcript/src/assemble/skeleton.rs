//! Skeleton building: merge copies by uuid (§3.4 step 1), effective parents (step 2 + A1),
//! then the derived tree facts (children, subtree maxima, anchors, heads, Main Line).

use std::collections::{BTreeSet, HashMap};

use super::{LoadedFile, SessionSkeleton, SkeletonNode, prompt, tree};
use crate::model::{PromptOrigin, TokenTotals};
use crate::raw::RawEntry;

/// Per-file facts collected during the merge, resolved after all files are read.
#[derive(Default)]
struct FileFacts {
    last_leaves: Vec<String>,
    custom_title: Option<String>,
    ai_title: Option<String>,
}

/// Raw parent references of a node, resolved once the whole map is known.
struct ParentRefs {
    parent_uuid: Option<String>,
    logical_parent_uuid: Option<String>,
    /// Nearest preceding uuid-bearing entry of the same file (A1 fallback).
    prev_in_file: Option<usize>,
}

pub fn build(files: &[LoadedFile]) -> SessionSkeleton {
    // Largest file first (size ≈ offset of the last record); stable for equal sizes.
    let mut ranked: Vec<usize> = (0..files.len()).collect();
    ranked.sort_by_key(|&i| {
        std::cmp::Reverse(files[i].records.last().map(|r| r.offset).unwrap_or(0))
    });

    let mut s = SessionSkeleton::default();
    let mut refs: Vec<ParentRefs> = Vec::new();
    let mut facts: Vec<FileFacts> = Vec::with_capacity(files.len());
    let mut versions = BTreeSet::new();

    for (rank, &fi) in ranked.iter().enumerate() {
        let mut f = FileFacts::default();
        let mut prev: Option<usize> = None;
        for (ri, rec) in files[fi].records.iter().enumerate() {
            let e = &rec.entry;
            if let Some(v) = e.version.as_deref().filter(|v| !v.is_empty())
                && !versions.contains(v)
            {
                versions.insert(v.to_owned());
            }
            match e.entry_type() {
                "last-prompt" => {
                    if let Some(l) = &e.leaf_uuid {
                        f.last_leaves.push(l.clone());
                        s.last_prompt_leaves.push(l.clone());
                    }
                }
                "custom-title" => {
                    if let Some(t) = e.custom_title.as_deref().filter(|t| !t.trim().is_empty()) {
                        f.custom_title = Some(t.trim().to_owned());
                    }
                }
                "ai-title" => {
                    if let Some(t) = e.ai_title.as_deref().filter(|t| !t.trim().is_empty()) {
                        f.ai_title = Some(t.trim().to_owned());
                    }
                }
                _ => {}
            }
            let Some(uuid) = e.uuid.as_deref() else {
                continue;
            };
            if let Some(&existing) = s.by_uuid.get(uuid) {
                if s.nodes[existing].file as usize == fi {
                    s.duplicate_uuids += 1;
                }
                prev = Some(existing);
                continue;
            }
            let idx = s.nodes.len();
            s.nodes
                .push(node_of(e, uuid, fi, ri, rank as u32, rec.offset));
            s.by_uuid.insert(uuid.to_owned(), idx);
            refs.push(ParentRefs {
                parent_uuid: e.parent_uuid.clone(),
                logical_parent_uuid: e.logical_parent_uuid.clone(),
                prev_in_file: prev,
            });
            prev = Some(idx);
            if s.cwd.is_none() {
                s.cwd = e.cwd.clone().filter(|c| !c.is_empty());
            }
            if let Some(b) = e.git_branch.as_deref().filter(|b| !b.is_empty()) {
                s.git_branch = Some(b.to_owned());
            }
            if s.fork_origin_id.is_none() {
                s.fork_origin_id = e.forked_from.as_ref().and_then(|f| f.session_id.clone());
            }
        }
        facts.push(f);
    }
    s.versions = versions.into_iter().collect();
    s.custom_title = facts.iter().find_map(|f| f.custom_title.clone());
    s.ai_title = facts.iter().find_map(|f| f.ai_title.clone());

    // First human prompt in title form.
    for (i, n) in s.nodes.iter().enumerate() {
        if n.is_human {
            let e = s.entry(files, i);
            let text = prompt::user_text(e);
            let origin = prompt::classify(e, &text);
            s.first_prompt = Some(prompt::title_text(&origin, &text));
            break;
        }
    }

    resolve_parents(&mut s, &refs);
    index_messages(&mut s);
    tree::derive(&mut s);

    let leaf = choose_leaf(&s, &facts);
    if let Some(leaf) = leaf {
        let leaf = tree::descend(&s, leaf);
        s.main_path = tree::root_path(&s, leaf);
    }
    s.on_main = vec![false; s.nodes.len()];
    for &i in &s.main_path {
        s.on_main[i] = true;
    }
    s
}

fn node_of(
    e: &RawEntry,
    uuid: &str,
    file: usize,
    record: usize,
    rank: u32,
    offset: u64,
) -> SkeletonNode {
    let entry_type = e.entry_type().to_owned();
    let msg = e.message.as_ref();
    let is_tool_result = entry_type == "user" && e.is_tool_result();
    let is_compact_summary = e.is_compact_summary == Some(true);
    let mut is_meta = e.is_meta == Some(true);
    let mut is_human = false;
    if entry_type == "user" && !is_tool_result && !is_compact_summary {
        let text = prompt::user_text(e);
        let origin = prompt::classify(e, &text);
        // An empty command output has nothing to show; treat it like meta.
        is_meta |= origin == PromptOrigin::Meta
            || (origin == PromptOrigin::CommandOutput && prompt::is_empty_output(&text));
        is_human = prompt::is_human(e, &origin);
    }
    let mut tool_uses = Vec::new();
    let mut tool_results = Vec::new();
    for b in e.blocks() {
        match b.block_type() {
            "tool_use" => {
                if let Some(id) = &b.id {
                    tool_uses.push((id.clone(), b.name.clone().unwrap_or_default()));
                }
            }
            "tool_result" => {
                if let Some(id) = &b.tool_use_id {
                    tool_results.push(id.clone());
                }
            }
            _ => {}
        }
    }
    let is_assistant = entry_type == "assistant";
    SkeletonNode {
        uuid: uuid.to_owned(),
        parent: None,
        subtype: e.subtype.clone(),
        timestamp_ms: e.timestamp_ms(),
        message_id: if is_assistant {
            msg.and_then(|m| m.id.clone())
        } else {
            None
        },
        order: (rank, offset),
        is_meta,
        is_compact_summary,
        is_tool_result,
        forked: e.forked_from.is_some(),
        file: file as u32,
        record: record as u32,
        is_human,
        is_head: false,
        model: if is_assistant {
            msg.and_then(|m| m.model.clone())
        } else {
            None
        },
        usage: if is_assistant {
            msg.and_then(|m| m.usage.as_ref()).map(|u| TokenTotals {
                input: u.input_tokens.unwrap_or(0.0),
                output: u.output_tokens.unwrap_or(0.0),
                cache_read: u.cache_read_input_tokens.unwrap_or(0.0),
                cache_creation: u.cache_creation_input_tokens.unwrap_or(0.0),
            })
        } else {
            None
        },
        is_api_error: e.is_api_error_message == Some(true),
        tool_uses,
        tool_results,
        entry_type,
    }
}

/// §3.4 step 2 + A1 + A1b: `parentUuid` if in the map; else `logicalParentUuid` if in the map
/// and earlier in merged order; else, for a compact boundary or any entry with a dangling
/// `logicalParentUuid`, the nearest preceding uuid entry of the same file; else a root.
/// A final pass breaks any remaining parent cycle so every walk terminates.
fn resolve_parents(s: &mut SessionSkeleton, refs: &[ParentRefs]) {
    for (i, r) in refs.iter().enumerate() {
        let by_parent = r
            .parent_uuid
            .as_deref()
            .and_then(|u| s.by_uuid.get(u).copied())
            .filter(|&p| p != i);
        let parent = by_parent.or_else(|| {
            let logical = r
                .logical_parent_uuid
                .as_deref()
                .and_then(|u| s.by_uuid.get(u).copied())
                .filter(|&p| p < i);
            if logical.is_some() {
                logical
            } else if r.logical_parent_uuid.is_some() || s.nodes[i].is_compact_boundary() {
                r.prev_in_file
            } else {
                None
            }
        });
        s.nodes[i].parent = parent;
    }
    break_cycles(s);
}

/// Linear cycle breaker: walks each unvisited chain; a parent already on the current chain
/// closes a cycle, so the edge into it is cut (that node becomes a root).
fn break_cycles(s: &mut SessionSkeleton) {
    const NEW: u8 = 0;
    const ON_STACK: u8 = 1;
    const DONE: u8 = 2;
    let n = s.nodes.len();
    let mut state = vec![NEW; n];
    let mut chain = Vec::new();
    for start in 0..n {
        if state[start] != NEW {
            continue;
        }
        let mut cur = start;
        loop {
            state[cur] = ON_STACK;
            chain.push(cur);
            match s.nodes[cur].parent {
                Some(p) if state[p] == NEW => cur = p,
                Some(p) if state[p] == ON_STACK => {
                    s.nodes[cur].parent = None;
                    break;
                }
                _ => break,
            }
        }
        for &c in &chain {
            state[c] = DONE;
        }
        chain.clear();
    }
}

/// Fragments by `message.id`, tool_use / tool_result maps and head flags.
fn index_messages(s: &mut SessionSkeleton) {
    for (i, n) in s.nodes.iter().enumerate() {
        if let Some(m) = &n.message_id {
            s.fragments.entry(m.clone()).or_default().push(i);
        }
        for (id, _) in &n.tool_uses {
            s.tool_use_at.entry(id.clone()).or_insert(i);
        }
        for id in &n.tool_results {
            s.results_for.entry(id.clone()).or_default().push(i);
        }
    }
    for i in 0..s.nodes.len() {
        let n = &s.nodes[i];
        let head = n.is_human
            || (n.is_assistant()
                && match &n.message_id {
                    Some(m) => s.fragments[m][0] == i,
                    None => true,
                });
        s.nodes[i].is_head = head;
    }
}

/// §3.4 step 3: the leaf of the last `last-prompt` (of the largest file that has a valid one)
/// whose leaf is in the map; else the newest node by timestamp (ties: later merged order).
fn choose_leaf(s: &SessionSkeleton, facts: &[FileFacts]) -> Option<usize> {
    for f in facts {
        if let Some(i) = f.last_leaves.iter().rev().find_map(|l| s.by_uuid.get(l)) {
            return Some(*i);
        }
    }
    let mut best: Option<(f64, usize)> = None;
    for (i, n) in s.nodes.iter().enumerate() {
        let t = n.timestamp_ms.unwrap_or(f64::MIN);
        if best.is_none_or(|(bt, _)| t >= bt) {
            best = Some((t, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Fragment-merged token usage: max of each field per `message.id` (§3.4 step 10).
pub fn tokens_by_message(s: &SessionSkeleton) -> HashMap<&str, TokenTotals> {
    let mut out: HashMap<&str, TokenTotals> = HashMap::new();
    for n in &s.nodes {
        let (Some(m), Some(u)) = (n.message_id.as_deref(), n.usage.as_ref()) else {
            continue;
        };
        let t = out.entry(m).or_default();
        t.input = t.input.max(u.input);
        t.output = t.output.max(u.output);
        t.cache_read = t.cache_read.max(u.cache_read);
        t.cache_creation = t.cache_creation.max(u.cache_creation);
    }
    out
}
