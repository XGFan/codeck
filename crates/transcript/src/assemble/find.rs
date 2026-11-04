//! ⌘F within a Session: searches the full text of nodes, tool inputs and outputs.
//!
//! Walks the same nodes the transcript shows (scope, branch choices, hidden toggle). Text and
//! thinking come from the nodes; tool inputs are matched on their string values and tool outputs
//! on the full result text, read from `tool-results/<basename>` when persisted (≤ 8 MB).
//! Matching is case-insensitive (Unicode lowercase); counts are non-overlapping occurrences.

use std::borrow::Cow;
use std::io::Read;

use serde_json::Value;

use super::content::{self, DETAIL_BYTES};
use super::nodes::{persisted_ref, result_content};
use super::{AssembledSession, LoadedFile, SessionSkeleton, skeleton};
use crate::error::TranscriptResult;
use crate::model::{
    AssistantBlock, FindLocation, FindMatch, FindRequest, FindResult, NodeBody, TranscriptRequest,
    TranscriptScope,
};

pub fn find(
    s: &AssembledSession,
    r: &FindRequest,
    agent_file: Option<&LoadedFile>,
) -> TranscriptResult<FindResult> {
    let needle = r.query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(FindResult {
            matches: Vec::new(),
            total: 0,
        });
    }
    let t = super::transcript(
        s,
        &TranscriptRequest {
            session_id: r.session_id.clone(),
            scope: r.scope.clone(),
            branch_choices: r.branch_choices.clone(),
            include_hidden: r.include_hidden,
        },
        agent_file,
    )?;
    let (sk, files): (Cow<'_, SessionSkeleton>, &[LoadedFile]) = match (&r.scope, agent_file) {
        (TranscriptScope::Subagent { .. }, Some(f)) => {
            let files = std::slice::from_ref(f);
            (Cow::Owned(skeleton::build(files)), files)
        }
        _ => (Cow::Borrowed(&s.skeleton), &s.main_files),
    };
    let count = |text: &str| text.to_lowercase().matches(needle.as_str()).count() as u32;
    let mut matches = Vec::new();
    for node in &t.nodes {
        let mut m = NodeMatches::new(&node.id);
        match &node.body {
            NodeBody::UserPrompt { text, .. }
            | NodeBody::CompactSummary { text }
            | NodeBody::System { text, .. }
            | NodeBody::Attachment { text, .. } => m.add(FindLocation::Text, count(text)),
            NodeBody::Unknown { raw_json, .. } => m.add(FindLocation::Text, count(raw_json)),
            NodeBody::CompactBoundary { .. } => {}
            NodeBody::Assistant { blocks, .. } => {
                for b in blocks {
                    match b {
                        AssistantBlock::Text { text } => m.add(FindLocation::Text, count(text)),
                        AssistantBlock::Thinking { text } => {
                            m.add(FindLocation::Thinking, count(text))
                        }
                        AssistantBlock::Unknown { raw_json, .. } => {
                            m.add(FindLocation::Text, count(raw_json))
                        }
                        AssistantBlock::ToolCall(tc) => {
                            let id = &tc.tool_use_id;
                            m.add(
                                FindLocation::ToolInput {
                                    tool_use_id: id.clone(),
                                },
                                count(&tool_input(&sk, files, id)),
                            );
                            m.add(
                                FindLocation::ToolOutput {
                                    tool_use_id: id.clone(),
                                },
                                count(&tool_output(s, &sk, files, id)),
                            );
                        }
                    }
                }
            }
        }
        matches.extend(m.items);
    }
    let total = matches.iter().map(|m: &FindMatch| m.count).sum();
    Ok(FindResult { matches, total })
}

/// Matches of one node in display order; text and thinking counts merge into one entry each.
struct NodeMatches<'a> {
    node_id: &'a str,
    items: Vec<FindMatch>,
}

impl<'a> NodeMatches<'a> {
    fn new(node_id: &'a str) -> Self {
        NodeMatches {
            node_id,
            items: Vec::new(),
        }
    }

    fn add(&mut self, location: FindLocation, count: u32) {
        if count == 0 {
            return;
        }
        let mergeable = matches!(location, FindLocation::Text | FindLocation::Thinking);
        if mergeable && let Some(m) = self.items.iter_mut().find(|m| m.location == location) {
            m.count += count;
            return;
        }
        self.items.push(FindMatch {
            node_id: self.node_id.to_owned(),
            location,
            count,
        });
    }
}

/// String values of a tool input, joined by `\n`.
fn tool_input(sk: &SessionSkeleton, files: &[LoadedFile], id: &str) -> String {
    let Some(&i) = sk.tool_use_at.get(id) else {
        return String::new();
    };
    let input = sk
        .entry(files, i)
        .blocks()
        .iter()
        .find(|b| b.block_type() == "tool_use" && b.id.as_deref() == Some(id))
        .and_then(|b| b.input.as_deref())
        .and_then(|raw| serde_json::from_str::<Value>(raw.get()).ok());
    let mut out = Vec::new();
    if let Some(v) = &input {
        leaves(v, &mut out);
    }
    out.join("\n")
}

fn leaves<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| leaves(x, out)),
        Value::Object(o) => o.values().for_each(|x| leaves(x, out)),
        _ => {}
    }
}

/// Full output of a tool call: the persisted file when present, else the inline result text.
fn tool_output(
    s: &AssembledSession,
    sk: &SessionSkeleton,
    files: &[LoadedFile],
    id: &str,
) -> String {
    let found = sk.results_for.get(id).and_then(|rs| {
        rs.iter().find_map(|&ri| {
            let e = sk.entry(files, ri);
            e.blocks()
                .iter()
                .find(|b| b.block_type() == "tool_result" && b.tool_use_id.as_deref() == Some(id))
                .map(|b| (e, b))
        })
    });
    let Some((e, block)) = found else {
        return String::new();
    };
    let (inline, _) = result_content(block.content.as_deref());
    persisted_ref(e, &inline)
        .and_then(|p| content::persisted_path(&s.session_dir, &p.file_name))
        .and_then(|path| {
            let mut buf = Vec::new();
            std::fs::File::open(path)
                .ok()?
                .take(DETAIL_BYTES as u64)
                .read_to_end(&mut buf)
                .ok()?;
            Some(String::from_utf8_lossy(&buf).into_owned())
        })
        .unwrap_or(inline)
}
