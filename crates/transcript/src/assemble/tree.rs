//! Effective parents, children, leaf, Main Line path and conversational anchors (§3.4 steps 2–5).

use std::collections::{HashMap, HashSet};

use super::SessionSkeleton;
use crate::model::NodeId;

/// Fills `children`, `subtree_max`, `subtree_assistants`, `anchor` and `heads_by_anchor` from the
/// (cycle-free) effective parents. Linear in the number of nodes.
pub fn derive(s: &mut SessionSkeleton) {
    let n = s.nodes.len();
    let mut children = vec![Vec::new(); n];
    let mut roots = Vec::new();
    for (i, node) in s.nodes.iter().enumerate() {
        match node.parent {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }
    // Pre-order from the roots: every parent precedes its descendants.
    let mut pre = Vec::with_capacity(n);
    let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
    while let Some(v) = stack.pop() {
        pre.push(v);
        stack.extend(children[v].iter().rev());
    }

    let mut anchor = vec![None; n];
    for &v in &pre {
        if let Some(p) = s.nodes[v].parent {
            anchor[v] = if s.nodes[p].is_conversational() {
                Some(p)
            } else {
                anchor[p]
            };
        }
    }
    let mut subtree_max: Vec<usize> = (0..n).collect();
    let mut subtree_assistants: Vec<u32> = s
        .nodes
        .iter()
        .map(|x| u32::from(x.is_head && x.is_assistant()))
        .collect();
    for &v in pre.iter().rev() {
        if let Some(p) = s.nodes[v].parent {
            subtree_max[p] = subtree_max[p].max(subtree_max[v]);
            subtree_assistants[p] += subtree_assistants[v];
        }
    }
    let mut heads_by_anchor: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, node) in s.nodes.iter().enumerate() {
        if node.is_head {
            heads_by_anchor.entry(anchor[i]).or_default().push(i);
        }
    }
    s.children = children;
    s.subtree_max = subtree_max;
    s.subtree_assistants = subtree_assistants;
    s.anchor = anchor;
    s.heads_by_anchor = heads_by_anchor;
}

/// §3.4 step 3: from `v`, repeatedly move to the child whose subtree holds the newest entry.
pub fn descend(s: &SessionSkeleton, mut v: usize) -> usize {
    while let Some(&c) = s.children[v].iter().max_by_key(|&&c| s.subtree_max[c]) {
        v = c;
    }
    v
}

/// Root … `v` (ancestors reversed). Parents are cycle-free; the step bound is a second guard.
pub fn root_path(s: &SessionSkeleton, v: usize) -> Vec<usize> {
    let mut path = vec![v];
    let mut cur = v;
    while let Some(p) = s.nodes[cur].parent {
        if path.len() > s.nodes.len() {
            break;
        }
        path.push(p);
        cur = p;
    }
    path.reverse();
    path
}

/// `root_path(head)` followed by the newest-leaf descent below `head`.
pub fn path_through(s: &SessionSkeleton, head: usize) -> Vec<usize> {
    let mut path = root_path(s, head);
    let mut v = head;
    while let Some(&c) = s.children[v].iter().max_by_key(|&&c| s.subtree_max[c]) {
        path.push(c);
        v = c;
    }
    path
}

/// §3.4 step 5: path nodes in order, each followed by its satellites (attachment / system
/// children that are off the path, and their attachment / system descendants).
pub fn display_entries(s: &SessionSkeleton, path: &[usize]) -> Vec<usize> {
    let mut on_path = vec![false; s.nodes.len()];
    for &v in path {
        on_path[v] = true;
    }
    let mut out = Vec::with_capacity(path.len() * 2);
    let mut stack = Vec::new();
    for &v in path {
        out.push(v);
        stack.extend(
            s.children[v]
                .iter()
                .rev()
                .filter(|&&c| !on_path[c] && s.nodes[c].is_satellite_type()),
        );
        while let Some(c) = stack.pop() {
            out.push(c);
            stack.extend(
                s.children[c]
                    .iter()
                    .rev()
                    .filter(|&&g| !on_path[g] && s.nodes[g].is_satellite_type()),
            );
        }
    }
    out
}

/// The result node absorbed by tool_use `id`: prefer one in `shown`, else the first.
pub fn chosen_result(s: &SessionSkeleton, id: &str, shown: &[bool]) -> Option<usize> {
    let all = s.results_for.get(id)?;
    all.iter()
        .copied()
        .find(|&r| shown[r])
        .or(all.first().copied())
}

/// Every entry uuid → display node id (A3): fragments map to their first fragment, absorbed
/// tool_results to the assistant node holding the tool_use, everything else to itself.
pub fn display_ids(s: &SessionSkeleton) -> HashMap<String, NodeId> {
    let mut out = HashMap::with_capacity(s.nodes.len());
    for (i, n) in s.nodes.iter().enumerate() {
        let target = if n.is_assistant() {
            s.display_index(i)
        } else if n.is_tool_result {
            n.tool_results
                .iter()
                .find_map(|id| s.tool_use_at.get(id))
                .map(|&a| s.display_index(a))
                .unwrap_or(i)
        } else {
            i
        };
        out.insert(n.uuid.clone(), s.nodes[target].uuid.clone());
    }
    out
}

/// A2: uuids of every entry in the Main Line display set: every fragment of every merged display
/// node (including fragments off the raw path whose `message.id` is on it), every absorbed
/// tool_result entry and every satellite on the Main Line. Used for `msg_text.on_main_line`.
pub fn main_set(s: &SessionSkeleton) -> HashSet<String> {
    let display = display_entries(s, &s.main_path);
    let mut members = vec![false; s.nodes.len()];
    for &i in &display {
        members[i] = true;
        if let Some(frags) = s.nodes[i]
            .message_id
            .as_ref()
            .and_then(|m| s.fragments.get(m))
        {
            for &f in frags {
                members[f] = true;
            }
        }
    }
    let shown = members.clone();
    for (i, n) in s.nodes.iter().enumerate() {
        if !shown[i] {
            continue;
        }
        for (id, _) in &n.tool_uses {
            if let Some(r) = chosen_result(s, id, &shown) {
                members[r] = true;
            }
        }
    }
    s.nodes
        .iter()
        .zip(members)
        .filter(|(_, m)| *m)
        .map(|(n, _)| n.uuid.clone())
        .collect()
}
