//! Session assembly: turns the append-mostly event log of a Session into normalized nodes
//! (plan §3.4). The engine loads files and sidecars; this module only computes.

pub mod branch;
pub mod content;
pub mod find;
pub mod jump;
pub mod nodes;
pub mod prompt;
pub mod skeleton;
pub mod subagent;
pub mod summarize;
pub mod tree;
pub mod workflow;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{TranscriptError, TranscriptResult};
use crate::model::{
    AgentId, FileRole, ForkOrigin, InheritedRange, NodeId, SubagentRun, TitleSource, TokenTotals,
    Transcript, TranscriptRequest, TranscriptScope, WorkflowRun,
};
use crate::parse::EntryRecord;
use crate::raw::RawEntry;

/// One parsed source file of a Session.
#[derive(Debug)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub role: FileRole,
    /// `None` for main files.
    pub agent_id: Option<String>,
    /// Records in file order.
    pub records: Vec<EntryRecord>,
}

/// Cheap in-memory uuid graph of a Session's main file(s) (§3.4 steps 1–4 and 7): enough to
/// compute aggregates, the Main Line, branch heads and `main_set` without building display nodes.
/// All `usize` values are indexes into `nodes`; per-node vectors are parallel to `nodes`.
#[derive(Clone, Debug, Default)]
pub struct SessionSkeleton {
    /// uuid-bearing entries after the merge (largest file first, first occurrence wins within a
    /// file, union by uuid across files), in merged file order.
    pub nodes: Vec<SkeletonNode>,
    /// uuid → index into `nodes`.
    pub by_uuid: HashMap<String, usize>,
    /// Duplicate uuids dropped by the merge (within one file; copies in other files are not counted).
    pub duplicate_uuids: u32,
    /// `leafUuid` of every `last-prompt` entry, in merged file order.
    pub last_prompt_leaves: Vec<String>,
    /// Last `custom-title` / `ai-title` values (of the largest file that has one).
    pub custom_title: Option<String>,
    pub ai_title: Option<String>,
    /// Main Line leaf → root, reversed (root first); empty for an empty Session.
    pub main_path: Vec<usize>,
    /// `true` for nodes on `main_path`.
    pub on_main: Vec<bool>,
    /// Children by effective parent, in merged file order.
    pub children: Vec<Vec<usize>>,
    /// Largest node index in each node's subtree (the "newest descendant" of §3.4 step 3).
    pub subtree_max: Vec<usize>,
    /// Assistant heads (distinct replies) in each node's subtree, the node itself included.
    pub subtree_assistants: Vec<u32>,
    /// Nearest conversational ancestor (§3.4 step 7 + A1); `None` = `"root"`.
    pub anchor: Vec<Option<usize>>,
    /// Branch heads grouped by anchor, in merged file order.
    pub heads_by_anchor: HashMap<Option<usize>, Vec<usize>>,
    /// Assistant `message.id` → fragment indexes in merged file order.
    pub fragments: HashMap<String, Vec<usize>>,
    /// tool_use id → node holding the tool_use block (first wins).
    pub tool_use_at: HashMap<String, usize>,
    /// tool_use id → nodes holding a matching tool_result block, in merged file order.
    pub results_for: HashMap<String, Vec<usize>>,
    /// First human/command prompt in title form (≤ 120 chars).
    pub first_prompt: Option<String>,
    /// First `cwd` seen.
    pub cwd: Option<String>,
    /// Last non-empty `gitBranch` seen.
    pub git_branch: Option<String>,
    /// Distinct Claude Code versions, sorted.
    pub versions: Vec<String>,
    /// `forkedFrom.sessionId` of the first fork-inherited entry.
    pub fork_origin_id: Option<String>,
}

/// One uuid-bearing entry in a [`SessionSkeleton`].
#[derive(Clone, Debug, Default)]
pub struct SkeletonNode {
    pub uuid: String,
    /// Effective parent per §3.4 step 2 + A1 (index into `SessionSkeleton::nodes`), cycle-free.
    pub parent: Option<usize>,
    pub entry_type: String,
    pub subtype: Option<String>,
    pub timestamp_ms: Option<f64>,
    /// Assistant `message.id`.
    pub message_id: Option<String>,
    /// Global file order (file rank in merge order, then byte offset).
    pub order: (u32, u64),
    /// `isMeta`, a user entry classified as meta (`<system-reminder>` …) or an empty command output.
    pub is_meta: bool,
    pub is_compact_summary: bool,
    pub is_tool_result: bool,
    /// Carries `forkedFrom` (fork-inherited).
    pub forked: bool,
    /// Where the winning record lives: index into the `files` slice given to `build_skeleton`.
    pub file: u32,
    /// Index into that file's `records`.
    pub record: u32,
    /// Human or command prompt (counts as a message, can head a Branch, supplies the title).
    pub is_human: bool,
    /// Branch head: a human/command prompt, or the first fragment of an assistant `message.id`.
    pub is_head: bool,
    pub model: Option<String>,
    pub usage: Option<TokenTotals>,
    pub is_api_error: bool,
    /// `(tool_use id, tool name)` of every tool_use block.
    pub tool_uses: Vec<(String, String)>,
    /// tool_use ids of every tool_result block.
    pub tool_results: Vec<String>,
}

impl SkeletonNode {
    pub fn is_assistant(&self) -> bool {
        self.entry_type == "assistant"
    }

    pub fn is_compact_boundary(&self) -> bool {
        self.entry_type == "system" && self.subtype.as_deref() == Some("compact_boundary")
    }

    /// Conversational anchor candidate (§3.4 step 7 + A1): assistant, non-meta user entries
    /// (prompts, tool results, compact summaries, notifications) and compact boundaries.
    pub fn is_conversational(&self) -> bool {
        match self.entry_type.as_str() {
            "assistant" => true,
            "user" => !self.is_meta,
            _ => self.is_compact_boundary(),
        }
    }

    /// Satellite candidate (§3.4 step 5).
    pub fn is_satellite_type(&self) -> bool {
        matches!(self.entry_type.as_str(), "attachment" | "system") && !self.is_compact_boundary()
    }

    /// Shown only with 「显示隐藏条目」: attachments, bookkeeping system entries (stop hooks, turn
    /// durations…), meta user entries and tool results not absorbed into their call.
    pub fn hidden_by_default(&self) -> bool {
        match self.entry_type.as_str() {
            "attachment" => true,
            "system" => {
                !self.is_compact_boundary()
                    && crate::known::is_hidden_system_subtype(self.subtype.as_deref().unwrap_or(""))
            }
            "user" => !self.is_compact_summary && (self.is_tool_result || self.is_meta),
            _ => false,
        }
    }
}

impl SessionSkeleton {
    /// The record of node `i` within `files` (the slice the skeleton was built from).
    pub fn entry<'a>(&self, files: &'a [LoadedFile], i: usize) -> &'a RawEntry {
        let n = &self.nodes[i];
        &files[n.file as usize].records[n.record as usize].entry
    }

    /// `(path, byte offset)` of node `i`'s source line.
    pub fn location<'a>(&self, files: &'a [LoadedFile], i: usize) -> (&'a Path, u64) {
        let n = &self.nodes[i];
        let f = &files[n.file as usize];
        (&f.path, f.records[n.record as usize].offset)
    }

    /// Display node id of node `i` (A3): the first fragment of a merged assistant message.
    pub fn display_index(&self, i: usize) -> usize {
        let n = &self.nodes[i];
        if n.is_assistant()
            && let Some(f) = n.message_id.as_ref().and_then(|m| self.fragments.get(m))
        {
            return f[0];
        }
        i
    }

    /// `anchor_key` of node `i`: the anchor uuid or `"root"`.
    pub fn anchor_key(&self, i: usize) -> String {
        match self.anchor[i] {
            Some(a) => self.nodes[a].uuid.clone(),
            None => "root".to_owned(),
        }
    }
}

/// Derived facts of a Session written to the `sessions` table (§3.4 steps 8–10).
/// Token and count figures are **main-file only** (A8); the engine adds subagent figures.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionAggregates {
    pub title: String,
    pub title_source: TitleSource,
    pub first_prompt: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub created_ms: Option<f64>,
    pub last_active_ms: f64,
    pub duration_ms: f64,
    /// Human/command prompts + distinct assistant `message.id`s.
    pub message_count: u32,
    pub tool_call_count: u32,
    /// Counted once per `message.id` (max of each usage field).
    pub tokens: TokenTotals,
    pub models: Vec<String>,
    pub primary_model: Option<String>,
    pub versions: Vec<String>,
    pub leaf_uuid: Option<String>,
    pub root_uuid: Option<String>,
    /// From `forkedFrom`.
    pub fork_origin_id: Option<String>,
    pub fork_point_uuid: Option<String>,
    pub is_empty: bool,
    pub duplicate_uuids: u32,
}

impl Default for SessionAggregates {
    fn default() -> Self {
        SessionAggregates {
            title: String::new(),
            title_source: TitleSource::Untitled,
            first_prompt: None,
            cwd: None,
            git_branch: None,
            created_ms: None,
            last_active_ms: 0.0,
            duration_ms: 0.0,
            message_count: 0,
            tool_call_count: 0,
            tokens: TokenTotals::default(),
            models: Vec::new(),
            primary_model: None,
            versions: Vec::new(),
            leaf_uuid: None,
            root_uuid: None,
            fork_origin_id: None,
            fork_point_uuid: None,
            is_empty: true,
            duplicate_uuids: 0,
        }
    }
}

/// Per-agent figures read by the engine from the index (`messages` / `tool_calls` / `subagents`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentStats {
    pub message_count: u32,
    pub tool_call_count: u32,
    pub tokens: TokenTotals,
    pub started_ms: Option<f64>,
    pub ended_ms: Option<f64>,
    pub model: Option<String>,
}

/// One `<sid>/subagents/**/agent-<id>.meta.json` (raw; parsed in `subagent.rs`).
#[derive(Clone, Debug, PartialEq)]
pub struct SubagentMetaFile {
    pub agent_id: AgentId,
    pub role: FileRole,
    /// Set for `<sid>/subagents/workflows/<runId>/agent-*`.
    pub workflow_run_id: Option<String>,
    /// The agent transcript (`agent-<id>.jsonl`), if present.
    pub transcript_path: Option<PathBuf>,
    /// Contents of the `.meta.json`, if present.
    pub meta_json: Option<String>,
}

/// One Workflow Run's sidecar files (raw; parsed in `workflow.rs`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowFiles {
    pub run_id: String,
    /// Contents of `<sid>/workflows/<runId>.json`, if present.
    pub workflow_json: Option<String>,
    /// Contents of `<sid>/subagents/workflows/<runId>/journal.jsonl`, if present.
    pub journal_jsonl: Option<String>,
}

/// Everything assembly needs besides the main files; gathered by the engine.
#[derive(Clone, Debug, Default)]
pub struct SessionSidecar {
    pub subagent_metas: Vec<SubagentMetaFile>,
    pub workflows: Vec<WorkflowFiles>,
    /// A8: per-agent figures from the index.
    pub agent_stats: HashMap<AgentId, AgentStats>,
    /// Origin Session from the index (via `forkedFrom`, or the shared-`root_uuid` fallback of §3.4 step 8).
    pub fork_origin: Option<ForkOrigin>,
}

/// A fully assembled Session, cached by the engine (LRU) and queried by `transcript`, `tool_detail`,
/// `image`, `find` and `jump`.
#[derive(Debug, Default)]
pub struct AssembledSession {
    pub session_id: String,
    /// `<project dir>/<session id>/` (holds `subagents/`, `tool-results/`, `workflows/`).
    pub session_dir: PathBuf,
    /// Set by the engine: Σ(size, mtime) of the source and sidecar files.
    pub revision: f64,
    /// Built from `main_files` in the order given (node `file` indexes refer to it).
    pub skeleton: SessionSkeleton,
    pub main_files: Vec<LoadedFile>,
    pub sidecar: SessionSidecar,
    pub subagents: Vec<SubagentRun>,
    pub workflows: Vec<WorkflowRun>,
    pub orphan_subagent_ids: Vec<AgentId>,
    /// Any entry uuid (fragment, absorbed tool_result, satellite) → display node id.
    pub display_id: HashMap<String, NodeId>,
    /// Main-file tool_use id → linked Subagent Run (not nested runs; those link in their parent's file).
    pub subagent_by_tool: HashMap<String, AgentId>,
    /// Main-file tool_use id → Workflow Run id.
    pub workflow_by_tool: HashMap<String, String>,
    /// tool_use id → uuid of the task-notification prompt that reports it.
    pub notification_by_tool: HashMap<String, NodeId>,
}

/// Builds the skeleton of the main file(s) (§3.4 steps 1–4).
pub fn build_skeleton(main_files: &[LoadedFile]) -> SessionSkeleton {
    skeleton::build(main_files)
}

/// Computes main-file aggregates (§3.4 steps 8–10).
pub fn summarize(s: &SessionSkeleton) -> SessionAggregates {
    summarize::summarize(s)
}

/// Assembles a Session for display.
pub fn assemble(
    session_id: &str,
    session_dir: &Path,
    main_files: Vec<LoadedFile>,
    sidecar: SessionSidecar,
) -> AssembledSession {
    let skeleton = skeleton::build(&main_files);
    let links = subagent::link(&skeleton, &main_files, &sidecar);
    let (workflows, workflow_by_tool) = workflow::runs(&skeleton, &main_files, &sidecar);
    let display_id = tree::display_ids(&skeleton);
    AssembledSession {
        session_id: session_id.to_owned(),
        session_dir: session_dir.to_path_buf(),
        revision: 0.0,
        skeleton,
        main_files,
        sidecar,
        subagents: links.runs,
        workflows,
        orphan_subagent_ids: links.orphans,
        display_id,
        subagent_by_tool: links.by_tool,
        workflow_by_tool,
        notification_by_tool: links.notification_by_tool,
    }
}

/// Produces the transcript for the requested scope and branch choices. `agent_file` is the
/// Subagent Run's file when `r.scope` is `Subagent`.
pub fn transcript(
    s: &AssembledSession,
    r: &TranscriptRequest,
    agent_file: Option<&LoadedFile>,
) -> TranscriptResult<Transcript> {
    match &r.scope {
        TranscriptScope::Main => {
            let sk = &s.skeleton;
            let path = branch::resolve_path(sk, &r.branch_choices);
            let display = tree::display_entries(sk, &path);
            // Index fallback (§3.4 step 8): no `forkedFrom` in the file, origin known from the index.
            let fork_point = if sk.fork_origin_id.is_none() {
                s.sidecar
                    .fork_origin
                    .as_ref()
                    .and_then(|o| o.fork_point_id.as_ref())
                    .and_then(|id| sk.by_uuid.get(id).copied())
            } else {
                None
            };
            let ctx = nodes::RenderCtx {
                skeleton: sk,
                files: &s.main_files,
                include_hidden: r.include_hidden,
                subagent_by_tool: &s.subagent_by_tool,
                workflow_by_tool: &s.workflow_by_tool,
                notification_by_tool: &s.notification_by_tool,
                fork_point,
            };
            let rendered = nodes::render(&ctx, &display);
            let branch_points = branch::branch_points(sk, &s.main_files, &path);
            let inherited = inherited_range(sk, &rendered.nodes, &s.sidecar);
            Ok(Transcript {
                session_id: s.session_id.clone(),
                scope: r.scope.clone(),
                revision: s.revision,
                nodes: rendered.nodes,
                branch_points,
                hidden_count: rendered.hidden_count,
                inherited,
                subagents: s.subagents.clone(),
                workflows: s.workflows.clone(),
                orphan_subagent_ids: s.orphan_subagent_ids.clone(),
            })
        }
        TranscriptScope::Subagent { agent_id } => {
            let file = agent_file.ok_or_else(|| {
                TranscriptError::NotFound(format!("Subagent {agent_id} 的记录文件"))
            })?;
            let files = std::slice::from_ref(file);
            let sk = skeleton::build(files);
            let path = branch::resolve_path(&sk, &r.branch_choices);
            let display = tree::display_entries(&sk, &path);
            let mut nested: Vec<SubagentRun> = s
                .subagents
                .iter()
                .filter(|run| run.parent_agent_id.as_deref() == Some(agent_id.as_str()))
                .cloned()
                .collect();
            let by_tool = subagent::nested_links(&sk, files, &nested);
            let notifications = subagent::nested_outcomes(&sk, files, &mut nested, &by_tool);
            let no_workflows = HashMap::new();
            let ctx = nodes::RenderCtx {
                skeleton: &sk,
                files,
                include_hidden: r.include_hidden,
                subagent_by_tool: &by_tool,
                workflow_by_tool: &no_workflows,
                notification_by_tool: &notifications,
                fork_point: None,
            };
            let rendered = nodes::render(&ctx, &display);
            let branch_points = branch::branch_points(&sk, files, &path);
            Ok(Transcript {
                session_id: s.session_id.clone(),
                scope: r.scope.clone(),
                revision: s.revision,
                nodes: rendered.nodes,
                branch_points,
                hidden_count: rendered.hidden_count,
                inherited: None,
                subagents: nested,
                workflows: Vec::new(),
                orphan_subagent_ids: Vec::new(),
            })
        }
    }
}

/// §3.4 step 8: the fork-inherited prefix. Nodes carrying `forkedFrom` are inherited; without
/// `forkedFrom`, the index fallback (`sidecar.fork_origin`) marks the path up to its fork point.
fn inherited_range(
    sk: &SessionSkeleton,
    nodes: &[crate::model::Node],
    sidecar: &SessionSidecar,
) -> Option<InheritedRange> {
    let origin_id = sk
        .fork_origin_id
        .clone()
        .or_else(|| sidecar.fork_origin.as_ref().map(|o| o.session_id.clone()))?;
    let origin_title = sidecar
        .fork_origin
        .as_ref()
        .filter(|o| o.session_id == origin_id)
        .and_then(|o| o.title.clone());
    let last = nodes.iter().rev().find(|n| n.inherited)?;
    let count = nodes.iter().filter(|n| n.inherited).count() as u32;
    Some(InheritedRange {
        origin_session_id: origin_id,
        origin_title,
        last_inherited_id: last.id.clone(),
        count,
    })
}
