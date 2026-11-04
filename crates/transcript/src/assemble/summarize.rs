//! Title, counts, tokens, fork origin, git branch (§3.4 steps 8–10).

use std::collections::HashMap;

use super::skeleton::tokens_by_message;
use super::{SessionAggregates, SessionSkeleton};
use crate::model::{TitleSource, TokenTotals};

/// Model name Claude Code writes on locally synthesized (error) messages.
const SYNTHETIC_MODEL: &str = "<synthetic>";

pub fn summarize(s: &SessionSkeleton) -> SessionAggregates {
    let (title, title_source) = if let Some(t) = &s.custom_title {
        (t.clone(), TitleSource::Custom)
    } else if let Some(t) = &s.ai_title {
        (t.clone(), TitleSource::Ai)
    } else if let Some(t) = &s.first_prompt {
        (t.clone(), TitleSource::FirstPrompt)
    } else {
        (String::new(), TitleSource::Untitled)
    };
    let title = super::prompt::truncate_chars(&title, super::prompt::TITLE_CHARS);

    let mut created: Option<f64> = None;
    let mut last: Option<f64> = None;
    let mut humans = 0u32;
    let mut tool_calls = 0u32;
    let mut no_id_assistants = 0u32;
    let mut tokens = TokenTotals::default();
    // model → (distinct messages, latest merged index)
    let mut models: HashMap<&str, (u32, usize)> = HashMap::new();
    for (i, n) in s.nodes.iter().enumerate() {
        if let Some(t) = n.timestamp_ms {
            created = Some(created.map_or(t, |c| c.min(t)));
            last = Some(last.map_or(t, |l| l.max(t)));
        }
        humans += u32::from(n.is_human);
        tool_calls += n.tool_uses.len() as u32;
        if !n.is_assistant() {
            continue;
        }
        if n.message_id.is_none() {
            no_id_assistants += 1;
            if let Some(u) = &n.usage {
                add(&mut tokens, u);
            }
        }
        if n.is_head
            && let Some(m) = n
                .model
                .as_deref()
                .filter(|m| *m != SYNTHETIC_MODEL && !m.is_empty())
        {
            let e = models.entry(m).or_insert((0, i));
            e.0 += 1;
            e.1 = i;
        }
    }
    let per_message = tokens_by_message(s);
    for u in per_message.values() {
        add(&mut tokens, u);
    }
    let message_count = humans + s.fragments.len() as u32 + no_id_assistants;

    let mut model_list: Vec<(&str, (u32, usize))> = models.into_iter().collect();
    // Most messages first; ties: most recently used.
    model_list.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(b.1.1.cmp(&a.1.1)));
    let primary_model = model_list.first().map(|(m, _)| (*m).to_owned());
    let mut models: Vec<String> = model_list.iter().map(|(m, _)| (*m).to_owned()).collect();
    models.sort();

    // Last inherited entry on the Main Line, else the last inherited entry at all.
    let fork_point_uuid = s
        .main_path
        .iter()
        .rev()
        .map(|&i| &s.nodes[i])
        .find(|n| n.forked)
        .or_else(|| s.nodes.iter().rev().find(|n| n.forked))
        .map(|n| n.uuid.clone());

    SessionAggregates {
        title,
        title_source,
        first_prompt: s.first_prompt.clone(),
        cwd: s.cwd.clone(),
        git_branch: s.git_branch.clone(),
        created_ms: created,
        last_active_ms: last.unwrap_or(0.0),
        duration_ms: match (created, last) {
            (Some(c), Some(l)) => l - c,
            _ => 0.0,
        },
        message_count,
        tool_call_count: tool_calls,
        tokens,
        models,
        primary_model,
        versions: s.versions.clone(),
        leaf_uuid: s.main_path.last().map(|&i| s.nodes[i].uuid.clone()),
        root_uuid: s.main_path.first().map(|&i| s.nodes[i].uuid.clone()),
        fork_origin_id: s.fork_origin_id.clone(),
        fork_point_uuid,
        is_empty: message_count == 0,
        duplicate_uuids: s.duplicate_uuids,
    }
}

fn add(t: &mut TokenTotals, u: &TokenTotals) {
    t.input += u.input;
    t.output += u.output;
    t.cache_read += u.cache_read;
    t.cache_creation += u.cache_creation;
}
