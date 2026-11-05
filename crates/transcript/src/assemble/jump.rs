//! Resolves a search hit to a display target (scope, branch choices, agent path, hidden flag).
//!
//! The hit's entry uuid (any fragment, absorbed tool_result or satellite; empty for a persisted
//! `.txt` hit, which is located by `tool_use_id`) maps to its display node (a merged assistant message's first fragment). Branch choices
//! select every Branch head on the target's root path that the default path does not follow.

use super::{AssembledSession, LoadedFile, SessionSkeleton, branch, skeleton, tree};
use crate::error::{TranscriptError, TranscriptResult};
use crate::model::{BranchChoice, JumpRequest, JumpTarget, TranscriptScope};
use crate::parse;

/// Bound on the `parentAgentId` chain (cycle guard).
const MAX_AGENT_DEPTH: usize = 64;

pub fn jump(s: &AssembledSession, r: &JumpRequest) -> TranscriptResult<JumpTarget> {
    let Some(agent_id) = &r.agent_id else {
        let t = locate(&s.skeleton, r)?;
        return Ok(JumpTarget {
            session_id: s.session_id.clone(),
            scope: TranscriptScope::Main,
            in_abandoned_branch: !t.choices.is_empty(),
            branch_choices: t.choices,
            node_id: t.node_id,
            tool_use_id: t.tool_use_id,
            agent_path: Vec::new(),
            in_subagent: false,
            needs_hidden: t.hidden,
        });
    };
    let meta = s
        .sidecar
        .subagent_metas
        .iter()
        .find(|m| &m.agent_id == agent_id)
        .ok_or_else(|| TranscriptError::NotFound("该 Subagent".to_owned()))?;
    let path = meta
        .transcript_path
        .as_ref()
        .ok_or_else(|| TranscriptError::NotFound(format!("Subagent {agent_id} 的记录文件")))?;
    let file = LoadedFile {
        path: path.clone(),
        role: meta.role,
        agent_id: Some(agent_id.clone()),
        records: parse::read_entries(path, 0)?.records,
    };
    let sk = skeleton::build(std::slice::from_ref(&file));
    let t = locate(&sk, r)?;
    Ok(JumpTarget {
        session_id: s.session_id.clone(),
        scope: TranscriptScope::Subagent {
            agent_id: agent_id.clone(),
        },
        in_abandoned_branch: !t.choices.is_empty(),
        branch_choices: t.choices,
        node_id: t.node_id,
        tool_use_id: t.tool_use_id,
        agent_path: agent_path(s, agent_id),
        in_subagent: true,
        needs_hidden: t.hidden,
    })
}

/// Outermost → innermost Subagent Run ids ending at `agent_id` (via `parentAgentId`).
fn agent_path(s: &AssembledSession, agent_id: &str) -> Vec<String> {
    let mut path = vec![agent_id.to_owned()];
    let mut cur = agent_id.to_owned();
    while path.len() < MAX_AGENT_DEPTH {
        let Some(parent) = s
            .subagents
            .iter()
            .find(|run| run.agent_id == cur)
            .and_then(|run| run.parent_agent_id.clone())
        else {
            break;
        };
        if path.contains(&parent) {
            break;
        }
        path.push(parent.clone());
        cur = parent;
    }
    path.reverse();
    path
}

struct Located {
    node_id: String,
    tool_use_id: Option<String>,
    choices: Vec<BranchChoice>,
    hidden: bool,
}

fn locate(sk: &SessionSkeleton, r: &JumpRequest) -> TranscriptResult<Located> {
    let by_tool = |t: &String| {
        sk.results_for
            .get(t)
            .and_then(|v| v.first().copied())
            .or_else(|| sk.tool_use_at.get(t).copied())
    };
    let idx = sk
        .by_uuid
        .get(&r.node_id)
        .copied()
        .or_else(|| r.tool_use_id.as_ref().and_then(by_tool))
        .ok_or_else(|| TranscriptError::NotFound("该消息".to_owned()))?;
    let n = &sk.nodes[idx];
    let mut tool_use_id = r.tool_use_id.clone();
    // `on`: the entry that must be on the selected path; `display`: the node the UI shows.
    let mut on = idx;
    if n.is_tool_result {
        let tid = tool_use_id
            .clone()
            .filter(|t| n.tool_results.contains(t))
            .or_else(|| n.tool_results.first().cloned());
        if let Some((t, &at)) = tid.and_then(|t| sk.tool_use_at.get(&t).map(|a| (t, a))) {
            tool_use_id = Some(t);
            on = at;
        }
    }
    let display = sk.display_index(on);
    Ok(Located {
        node_id: sk.nodes[display].uuid.clone(),
        tool_use_id,
        choices: choices_for(sk, on),
        hidden: sk.nodes[display].hidden_by_default(),
    })
}

/// Branch choices that put `target`'s root path on the selected path: every head on it with
/// sibling heads that the path resolved so far does not already follow.
fn choices_for(sk: &SessionSkeleton, target: usize) -> Vec<BranchChoice> {
    let mut choices = Vec::new();
    let mut on_path = vec![false; sk.nodes.len()];
    let mark = |on_path: &mut [bool], path: &[usize]| {
        on_path.fill(false);
        for &i in path {
            on_path[i] = true;
        }
    };
    mark(&mut on_path, &sk.main_path);
    for v in tree::root_path(sk, target) {
        let n = &sk.nodes[v];
        let siblings = sk.heads_by_anchor.get(&sk.anchor[v]).map_or(0, Vec::len);
        if !n.is_head || siblings < 2 || on_path[v] {
            continue;
        }
        choices.push(BranchChoice {
            anchor_key: sk.anchor_key(v),
            head_id: n.uuid.clone(),
        });
        mark(&mut on_path, &branch::resolve_path(sk, &choices));
    }
    choices
}
