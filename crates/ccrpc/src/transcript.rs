//! Incremental tail of a Session's transcript (main file, Subagent Run files, Workflow Run files)
//! producing Blocks and the few markers the state machine needs. Reads only complete lines.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use ::transcript::assemble::{
    LoadedFile, WorkflowFiles, content, nodes, prompt, skeleton, subagent, tree, workflow,
};
use ::transcript::layout::{self, AgentFile};
use ::transcript::model::FileRole;
use ::transcript::parse::{ReadOptions, read_entries_with};
use ::transcript::raw::{RawBlock, RawContent, RawEntry, decode_str};
use serde::Serialize;
use serde_json::{Value, json};

/// Cap on one read of a transcript file.
const MAX_READ: u64 = 8 * 1024 * 1024;

/// Cap on text carried by one block event (tool outputs can be huge).
pub const MAX_BLOCK_TEXT: usize = 100 * 1024;

/// A Subagent file unchanged this long is checked only every [`COLD_CHECK`].
const COLD_AFTER: Duration = Duration::from_secs(60);
const COLD_CHECK: Duration = Duration::from_secs(5);
/// Directory listings (new Subagent / Workflow files) at most this often.
const DISCOVER_EVERY: Duration = Duration::from_millis(500);

/// A tool output Claude Code saved to `tool-results/` (the transcript keeps a preview).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Persisted {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    User,
    Text,
    Thinking,
    ToolUse,
    ToolResult,
}

/// One content unit of a transcript message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Block {
    pub kind: BlockKind,
    /// uuid of the transcript entry holding it.
    pub uuid: String,
    /// Position within the entry's content array.
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// tool_use id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// tool_result whose full output is in a file (`content` is Claude Code's preview).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persisted: Option<Persisted>,
    /// User input delivered mid-turn (typed while busy).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub queued: bool,
    /// A prompt's images (media types, in order); their data comes from `get_image`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Set when the block comes from a Subagent Run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    /// Set when the Subagent Run belongs to a Workflow Run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

impl Block {
    fn new(kind: BlockKind, uuid: &str, index: usize) -> Block {
        Block {
            kind,
            uuid: uuid.to_string(),
            index,
            text: None,
            id: None,
            name: None,
            input: None,
            tool_use_id: None,
            content: None,
            is_error: None,
            persisted: None,
            queued: false,
            images: Vec::new(),
            agent_id: None,
            parent_tool_use_id: None,
            run_id: None,
        }
    }
}

/// What a poll found, in transcript order.
#[derive(Debug, Clone, PartialEq)]
pub enum TxEvent {
    Block(Box<Block>),
    /// `system` / `turn_duration` in the main file: the end of a turn, with its duration.
    TurnEnd {
        duration_ms: Option<u64>,
    },
    /// One assistant message's token counts (once per message id; main line or Subagent Run).
    Usage {
        /// From the main file (not a Subagent Run).
        main: bool,
        model: Option<String>,
        /// The effort level the reply ran at, when Claude Code recorded one.
        effort: Option<String>,
        usage: ::transcript::pricing::Usage,
        /// Cost Claude Code logged itself, if any.
        logged_cost: Option<f64>,
    },
    /// The API failed in the main line (`kind` as Claude Code records it, e.g. `server_error`).
    ApiError {
        kind: Option<String>,
        message: String,
    },
    /// `queue-operation` / `enqueue`: input typed while busy was accepted.
    Enqueued {
        content: String,
    },
    /// Claude Code's input queue changed (main line): what waits in it now, oldest first, task
    /// notifications included.
    Queue(Vec<String>),
    /// `queue-operation` / `popAll`: `↑` in the TUI took this item back into the input box (one
    /// record per item taken; what cannot be edited there stays queued).
    Popped {
        content: String,
    },
    /// A user prompt entry in the main file (string content), or a slash command's record there as
    /// typed (`/name args`; no block, it only confirms the `prompt` that sent it).
    UserPrompt {
        uuid: String,
        text: String,
        /// Image blocks the prompt carries.
        images: usize,
    },
    /// The Main Line moved to another Branch: blocks emitted before this event that come after
    /// `at_uuid` are off it (`None`: the new Branch starts a new root, all earlier blocks are off).
    Branch {
        at_uuid: Option<String>,
    },
    /// A Workflow Run's state changed (the whole state, protocol shape).
    Workflow(Value),
    /// A local `/model` or `/effort` (run from here or typed in the TUI) said what it set, in the
    /// main file: the model's display name (`Opus 4.7`) and / or the effort level.
    Setting {
        model: Option<String>,
        effort: Option<String>,
    },
}

/// What a `/model` or `/effort` output says it set: `Set model to \`Opus 4.7\` and saved …`,
/// `Kept model as Opus 4.7`, `Set model to Opus 4.7 (default) …`, `Set effort level to low (…`.
pub fn local_setting(s: &str) -> Option<(Option<String>, Option<String>)> {
    let t = s.trim();
    let t = t.strip_prefix("<local-command-stdout>").unwrap_or(t);
    let t = t
        .split("</local-command-stdout>")
        .next()
        .unwrap_or(t)
        .trim();
    if let Some(rest) = t
        .strip_prefix("Set model to ")
        .or_else(|| t.strip_prefix("Kept model as "))
    {
        let name = rest
            .split(" and saved")
            .next()
            .unwrap_or(rest)
            .lines()
            .next()
            .unwrap_or("");
        let name = name.replace('`', "");
        let name = name.trim().trim_end_matches("(default)").trim();
        return (!name.is_empty()).then(|| (Some(name.to_string()), None));
    }
    let rest = t.strip_prefix("Set effort level to ")?;
    let level: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    (!level.is_empty()).then(|| (None, Some(level.to_lowercase())))
}

/// What a background task runs; on the wire `agent`, `shell`, `monitor` or `workflow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Agent,
    Shell,
    /// The Monitor tool's watch, which notifies on each event (Claude Code runs it as a background
    /// shell, and Stop's `background_tasks` calls it `shell`).
    Monitor,
    Workflow,
}

impl TaskKind {
    /// The kind as [`subagent::background_launch`] names it.
    pub fn from_name(name: &str) -> Option<TaskKind> {
        match name {
            "agent" => Some(TaskKind::Agent),
            "shell" => Some(TaskKind::Shell),
            "monitor" => Some(TaskKind::Monitor),
            "workflow" => Some(TaskKind::Workflow),
            _ => None,
        }
    }
}

/// A background task Claude Code started: a Subagent Run, a shell, a Monitor or a Workflow Run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BgTask {
    /// Claude Code's task id (agentId, background shell id, Workflow task id).
    pub id: String,
    pub kind: TaskKind,
    pub tool_use_id: String,
    pub description: Option<String>,
    /// The shell command of a background shell or a Monitor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `running` until Claude Code reports it ended (`completed`, `failed`, `killed`, …).
    pub status: String,
}

/// A tool_use that has no tool_result yet.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTool {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub agent_id: Option<String>,
    /// Arrival order.
    pub seq: u64,
}

#[derive(Debug)]
struct SubFile {
    path: PathBuf,
    offset: u64,
    parent_tool_use_id: Option<String>,
    /// Workflow Run the Subagent Run belongs to.
    run_id: Option<String>,
    /// Last time the file grew.
    changed_at: Instant,
    checked_at: Option<Instant>,
}

/// What a main-file entry that produced blocks was, for the resend rule of Branches.
#[derive(Debug, Clone, PartialEq)]
enum Emitted {
    Prompt(String),
    /// `[Request interrupted by user…]`.
    Interruption,
    Other,
}

/// What was last seen of a Workflow Run's files.
#[derive(Debug, Default)]
struct WorkflowSeen {
    sig: Vec<(u64, Option<SystemTime>)>,
    tool_use_id: Option<String>,
    last: Option<Value>,
}

#[derive(Debug)]
pub struct Tailer {
    main: PathBuf,
    offset: u64,
    subs: HashMap<String, SubFile>,
    pending: HashMap<String, PendingTool>,
    seq: u64,
    last_text: Option<String>,
    last_discover: Option<Instant>,
    /// The last poll hit the read cap on some file.
    capped: bool,
    /// Workflow Run id → its `Workflow` tool_use id (from the tool result's `runId`).
    run_tools: HashMap<String, String>,
    workflows: HashMap<String, WorkflowSeen>,
    /// tool_use id → file holding the full output.
    persisted: HashMap<String, PathBuf>,
    /// Main-file entries that produced blocks, in order.
    emitted: Vec<(String, Emitted)>,
    emitted_set: HashSet<String>,
    /// Parent of every main-file entry read so far (`logicalParentUuid` across a compact).
    parents: HashMap<String, Option<String>>,
    /// Read the main file only (no Subagent / Workflow files).
    main_only: bool,
    /// Assistant message ids whose usage was reported.
    usage_seen: HashSet<String>,
    /// Background tasks, in start order.
    tasks: Vec<BgTask>,
    /// Byte offset of every main-file entry, kept when `main_only` or `keep_offsets`.
    offsets: HashMap<String, u64>,
    keep_offsets: bool,
    /// Claude Code's input queue as the main file's `queue-operation` entries leave it.
    queue: Vec<String>,
    /// The main line's last `SendFeedback` call `(tool_use id, input)`: the draft behind the card
    /// Claude Code shows above the input box.
    last_feedback: Option<(String, Value)>,
    /// Entries copied from the Session this one continues (moved to the background): read for
    /// what they leave behind (pending tools, the tree, usage already counted), but they produce
    /// no events.
    copied: HashSet<String>,
    /// Still in the copied part at the start of the file: entries without a uuid count as copied.
    in_copy: bool,
}

/// Directory holding the Subagent Run files of `main` (`<main without .jsonl>/subagents`).
pub fn subagents_dir(main: &Path) -> PathBuf {
    layout::subagents_dir(&main.with_extension(""))
}

impl Tailer {
    /// Starts at the beginning; the caller decides what to do with the first poll (history).
    pub fn new(main: &Path) -> Tailer {
        Tailer {
            main: main.to_path_buf(),
            offset: 0,
            subs: HashMap::new(),
            pending: HashMap::new(),
            seq: 0,
            last_text: None,
            last_discover: None,
            capped: false,
            run_tools: HashMap::new(),
            workflows: HashMap::new(),
            persisted: HashMap::new(),
            emitted: Vec::new(),
            emitted_set: HashSet::new(),
            parents: HashMap::new(),
            main_only: false,
            offsets: HashMap::new(),
            keep_offsets: false,
            usage_seen: HashSet::new(),
            tasks: Vec::new(),
            queue: Vec::new(),
            last_feedback: None,
            copied: HashSet::new(),
            in_copy: false,
        }
    }

    /// This transcript starts with a copy of another Session's entries (`uuids`: that Session's;
    /// a conversation moved to the background): they were reported there and produce no events
    /// here, however late they are written.
    pub fn copied_from(&mut self, uuids: HashSet<String>) {
        self.in_copy = !uuids.is_empty();
        self.copied = uuids;
    }

    /// `entry` is one of the copied ones (see [`Tailer::copied_from`]).
    fn is_copied(&mut self, entry: &RawEntry) -> bool {
        match &entry.uuid {
            Some(u) if self.copied.contains(u) => true,
            Some(_) => {
                self.in_copy = false;
                false
            }
            None => self.in_copy,
        }
    }

    /// Claude Code's input queue now (main line), oldest first.
    pub fn queue(&self) -> &[String] {
        &self.queue
    }

    /// The queued item with `content` gone: by its text; failing that, by its text with the
    /// opening tag's attributes left out.
    fn unqueue(&mut self, content: &str) {
        let i = self
            .queue
            .iter()
            .position(|q| q.trim() == content.trim())
            .or_else(|| {
                let key = queue_key(content);
                self.queue.iter().position(|q| queue_key(q) == key)
            });
        if let Some(i) = i {
            self.queue.remove(i);
        }
    }

    /// Background tasks seen so far.
    pub fn tasks(&self) -> &[BgTask] {
        &self.tasks
    }

    /// A task ccrpc stopped: `stopped` until Claude Code's own notice says otherwise.
    pub fn mark_stopped(&mut self, id: &str) {
        if let Some(t) = self
            .tasks
            .iter_mut()
            .find(|t| t.id == id && t.status == "running")
        {
            t.status = "stopped".into();
        }
    }

    /// Keeps the byte offset of every main-file entry (for paging the history).
    pub fn keep_offsets(&mut self) {
        self.keep_offsets = true;
    }

    /// Byte offset of the main-file entry `uuid` (with `keep_offsets`).
    pub fn offset_of(&self, uuid: &str) -> Option<u64> {
        self.offsets.get(uuid).copied()
    }

    /// How far the main file has been read.
    pub fn read_to(&self) -> u64 {
        self.offset
    }

    /// Looks for new Subagent Run / Workflow files on the next poll (a Subagent just started or
    /// stopped).
    pub fn rediscover(&mut self) {
        self.last_discover = None;
    }

    /// Reports every Workflow Run again on the next look (its current state).
    pub fn forget_workflows(&mut self) {
        self.workflows.clear();
        self.last_discover = None;
    }

    /// `<sessionId>/` next to the main file: subagents, workflows, tool-results.
    fn session_dir(&self) -> PathBuf {
        self.main.with_extension("")
    }

    pub fn main_path(&self) -> &Path {
        &self.main
    }

    /// The main line's last `SendFeedback` call: `(tool_use id, input)`.
    pub fn last_feedback(&self) -> Option<&(String, Value)> {
        self.last_feedback.as_ref()
    }

    /// Last assistant text block seen in the main file.
    pub fn last_text(&self) -> Option<&str> {
        self.last_text.as_deref()
    }

    /// Tools still waiting for a result, oldest first.
    pub fn pending(&self) -> Vec<&PendingTool> {
        let mut v: Vec<_> = self.pending.values().collect();
        v.sort_by_key(|p| p.seq);
        v
    }

    /// Oldest pending tool named `name` (any agent).
    pub fn pending_named(&self, name: &str) -> Option<&PendingTool> {
        self.pending().into_iter().find(|p| p.name == name)
    }

    /// The pending tool with this id.
    pub fn pending_by_id(&self, id: &str) -> Option<&PendingTool> {
        self.pending.get(id)
    }

    /// The last `poll` stopped at the read cap: call it again to read on.
    pub fn behind(&self) -> bool {
        self.capped
    }

    pub fn poll(&mut self) -> Vec<TxEvent> {
        let mut out = Vec::new();
        self.capped = false;
        // A file that shrank was replaced: what was read from it starts over. What other files
        // gave (Subagent / Workflow progress, the Workflow Runs reported) and the usage already
        // reported carry over, so none of it is reported twice.
        if std::fs::metadata(&self.main).map(|m| m.len()).unwrap_or(0) < self.offset {
            let old = std::mem::replace(self, Tailer::new(&self.main));
            self.copied_from(old.copied);
            self.main_only = old.main_only;
            self.keep_offsets = old.keep_offsets;
            self.subs = old.subs;
            self.workflows = old.workflows;
            self.usage_seen = old.usage_seen;
            self.last_discover = old.last_discover;
        }
        if let Ok(chunk) = read_lines(&self.main, self.offset) {
            self.offset = chunk.consumed_to;
            self.capped |= chunk.capped;
            for l in &chunk.lines {
                if (self.main_only || self.keep_offsets)
                    && let Some(u) = &l.entry.uuid
                {
                    self.offsets.entry(u.clone()).or_insert(l.offset);
                }
                let copied = self.is_copied(&l.entry);
                // A Branch is reported before the blocks of its first entry.
                if !copied
                    && self.is_fork(l)
                    && let Some(at) = self.branch_moved(l)
                {
                    out.push(TxEvent::Branch { at_uuid: at });
                }
                let before = out.len();
                self.on_entry(l, None, None, None, &mut out);
                self.note_emitted(l, &out[before..]);
                if copied {
                    out.truncate(before);
                }
            }
        }
        if self.main_only {
            return out;
        }
        let discover = self
            .last_discover
            .is_none_or(|t| t.elapsed() >= DISCOVER_EVERY);
        if discover {
            self.last_discover = Some(Instant::now());
            self.discover_subs();
        }
        let mut ids: Vec<String> = self.subs.keys().cloned().collect();
        ids.sort();
        let now = Instant::now();
        for id in ids {
            let Some(sf) = self.subs.get_mut(&id) else {
                continue;
            };
            // Long-quiet files (finished Subagent Runs) are looked at less often.
            if now.duration_since(sf.changed_at) >= COLD_AFTER
                && sf
                    .checked_at
                    .is_some_and(|t| now.duration_since(t) < COLD_CHECK)
            {
                continue;
            }
            sf.checked_at = Some(now);
            let Ok(len) = std::fs::metadata(&sf.path).map(|m| m.len()) else {
                continue;
            };
            if len < sf.offset {
                sf.offset = 0;
            } else if len == sf.offset {
                continue;
            }
            sf.changed_at = now;
            let (path, offset) = (sf.path.clone(), sf.offset);
            let run = sf.run_id.clone();
            let parent = match &run {
                Some(r) => self.run_tools.get(r).cloned(),
                None => sf.parent_tool_use_id.clone(),
            };
            if let Ok(chunk) = read_lines(&path, offset) {
                if let Some(s) = self.subs.get_mut(&id) {
                    s.offset = chunk.consumed_to;
                }
                self.capped |= chunk.capped;
                for l in chunk.lines {
                    self.on_entry(&l, Some(&id), parent.as_deref(), run.as_deref(), &mut out);
                }
            }
        }
        if discover {
            self.poll_workflows(&mut out);
        }
        out
    }

    /// A user prompt may start a new Branch (rewind, edit and resend). Its ancestors are walked
    /// past entries that produced no block (attachments, system entries, hooks); reaching the
    /// last entry that did means it simply continues the conversation. Reaching an earlier one,
    /// or the root, makes it a candidate checked against the Main Line. An ancestor not read yet
    /// (Claude Code sometimes writes a child before its parent) means no verdict: not a fork.
    fn is_fork(&self, line: &Line) -> bool {
        // Only a person's prompt: background results and teammate messages hang off where they
        // were started, not off the end of the conversation.
        if prompt_text(line).is_none() || !prompt::is_human_prompt(&line.entry) {
            return false;
        }
        let Some(last) = self.emitted.last().map(|(u, _)| u) else {
            return false;
        };
        let e = &line.entry;
        let mut cur = e
            .parent_uuid
            .clone()
            .or_else(|| e.logical_parent_uuid.clone());
        for _ in 0..=self.parents.len() {
            let Some(u) = cur else {
                return true;
            };
            if self.emitted_set.contains(&u) {
                return &u != last;
            }
            match self.parents.get(&u) {
                Some(p) => cur = p.clone(),
                None => return false,
            }
        }
        false
    }

    /// Records the main-file entry behind blocks just produced (for Branch checks).
    fn note_emitted(&mut self, line: &Line, new: &[TxEvent]) {
        let e = &line.entry;
        let Some(uuid) = e.uuid.clone() else {
            return;
        };
        self.parents.insert(
            uuid.clone(),
            e.parent_uuid
                .clone()
                .or_else(|| e.logical_parent_uuid.clone()),
        );
        let mut kind = Emitted::Other;
        let mut any = false;
        for ev in new {
            match ev {
                TxEvent::Block(b) if b.agent_id.is_none() => {
                    any = true;
                    if b.kind == BlockKind::User
                        && b.text.as_deref().is_some_and(prompt::is_interruption)
                    {
                        kind = Emitted::Interruption;
                    }
                }
                TxEvent::UserPrompt { text, .. } => kind = Emitted::Prompt(text.clone()),
                _ => {}
            }
        }
        if any && self.emitted_set.insert(uuid.clone()) {
            self.emitted.push((uuid, kind));
        }
    }

    /// With the Main Line going through `fork`, the blocks already emitted that fell off it:
    /// `Some(at_uuid)` to report, `at_uuid` being the last emitted block entry still on the Main
    /// Line (`None`: none is). They are dropped from the record either way. A Branch that only
    /// resent the same prompt with no reply, an interruption note aside, is not reported. Only the
    /// file up to `fork` counts, so the verdict does not depend on when the file was read.
    fn branch_moved(&mut self, fork: &Line) -> Option<Option<String>> {
        let next_prompt = prompt_text(fork);
        let next_prompt = next_prompt.as_deref();
        let abandoned = off_main_uuids(&self.main, fork.entry.uuid.as_deref()?, fork.offset)?;
        let off: Vec<Emitted> = self
            .emitted
            .iter()
            .filter(|(u, _)| abandoned.contains(u))
            .map(|(_, k)| k.clone())
            .collect();
        if off.is_empty() {
            return None;
        }
        self.emitted.retain(|(u, _)| !abandoned.contains(u));
        self.emitted_set.retain(|u| !abandoned.contains(u));
        let resend = next_prompt.is_some_and(|p| {
            off.iter().any(|k| matches!(k, Emitted::Prompt(_)))
                && off.iter().all(|k| match k {
                    Emitted::Prompt(t) => t.trim() == p.trim(),
                    Emitted::Interruption => true,
                    Emitted::Other => false,
                })
        });
        (!resend).then(|| self.emitted.last().map(|(u, _)| u.clone()))
    }

    fn poll_workflows(&mut self, out: &mut Vec<TxEvent>) {
        let dir = self.session_dir();
        let json_dir = layout::workflows_dir(&dir);
        let journal_dir = layout::workflow_agents_dir(&dir);
        let mut runs: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&json_dir) {
            for e in rd.flatten() {
                if let Some(r) = e.file_name().to_str().and_then(layout::workflow_json_run) {
                    runs.push(r.to_string());
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(&journal_dir) {
            for e in rd.flatten() {
                if e.path().join(layout::JOURNAL).is_file()
                    && let Some(n) = e.file_name().to_str()
                {
                    runs.push(n.to_string());
                }
            }
        }
        runs.sort();
        runs.dedup();
        for run in runs {
            let json_path = json_dir.join(format!("{run}.json"));
            let journal_path = journal_dir.join(&run).join(layout::JOURNAL);
            let stat = |p: &Path| {
                std::fs::metadata(p)
                    .map(|m| (m.len(), m.modified().ok()))
                    .unwrap_or((0, None))
            };
            let sig = vec![stat(&json_path), stat(&journal_path)];
            let tool = self.run_tools.get(&run).cloned();
            let seen = self.workflows.entry(run.clone()).or_default();
            if seen.sig == sig && seen.tool_use_id == tool {
                continue;
            }
            seen.sig = sig;
            seen.tool_use_id = tool.clone();
            let files = WorkflowFiles {
                run_id: run.clone(),
                workflow_json: std::fs::read_to_string(&json_path).ok(),
                journal_jsonl: std::fs::read_to_string(&journal_path).ok(),
            };
            let r = workflow::run_from_files(&files);
            let v = json!({
                "type": "workflow",
                "run_id": r.run_id,
                "tool_use_id": tool,
                "name": r.name,
                "status": r.status,
                "phases": r.phases.iter().map(|p| json!({
                    "index": p.index, "title": p.title, "detail": p.detail,
                })).collect::<Vec<_>>(),
                "agents": r.agents.iter().map(|a| json!({
                    "agent_id": a.agent_id, "label": a.label, "phase_index": a.phase_index,
                    "state": a.state,
                })).collect::<Vec<_>>(),
            });
            if seen.last.as_ref() != Some(&v) {
                seen.last = Some(v.clone());
                out.push(TxEvent::Workflow(v));
            }
        }
    }

    fn discover_subs(&mut self) {
        let dir = self.session_dir();
        self.discover_in(&layout::subagents_dir(&dir), None);
        if let Ok(rd) = std::fs::read_dir(layout::workflow_agents_dir(&dir)) {
            for e in rd.flatten() {
                if let Some(run) = e.file_name().to_str().map(str::to_string) {
                    self.discover_in(&e.path(), Some(run));
                }
            }
        }
    }

    fn discover_in(&mut self, dir: &Path, run: Option<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(AgentFile::Transcript(agent_id)) = layout::agent_file(name) else {
                continue;
            };
            if let Some(s) = self.subs.get_mut(agent_id) {
                if s.parent_tool_use_id.is_none() && s.run_id.is_none() {
                    s.parent_tool_use_id = read_meta_tool_use_id(dir, agent_id);
                }
                continue;
            }
            let parent = if run.is_none() {
                read_meta_tool_use_id(dir, agent_id)
            } else {
                None
            };
            self.subs.insert(
                agent_id.to_string(),
                SubFile {
                    path: p.clone(),
                    offset: 0,
                    parent_tool_use_id: parent,
                    run_id: run.clone(),
                    changed_at: Instant::now(),
                    checked_at: None,
                },
            );
        }
    }

    /// Full output of a tool result: the saved file when Claude Code persisted it (capped like a
    /// block), otherwise the result text as the transcript has it.
    pub fn tool_result(&self, tool_use_id: &str) -> Option<(String, Option<Persisted>)> {
        if let Some(path) = self.persisted.get(tool_use_id) {
            let text = read_capped(path)?;
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            return Some((
                text,
                Some(Persisted {
                    path: path.display().to_string(),
                    size,
                }),
            ));
        }
        // Not persisted: read the transcript again (main file and Subagent Runs).
        let mut t = Tailer::new(&self.main);
        loop {
            for ev in t.poll() {
                if let TxEvent::Block(b) = ev
                    && b.kind == BlockKind::ToolResult
                    && b.tool_use_id.as_deref() == Some(tool_use_id)
                {
                    return Some((b.content.clone().unwrap_or_default(), None));
                }
            }
            if !t.behind() {
                return None;
            }
        }
    }

    /// Image `index` (0-based, among the entry's images) of the main-line prompt `uuid`:
    /// `(media_type, base64 data)`.
    pub fn image(&self, uuid: &str, index: usize) -> Option<(String, String)> {
        use std::io::BufRead;
        let needle = format!("\"uuid\":\"{uuid}\"");
        let f = std::io::BufReader::new(std::fs::File::open(&self.main).ok()?);
        for line in f.lines() {
            let line = line.ok()?;
            if !line.contains(&needle) {
                continue;
            }
            let v: Value = serde_json::from_str(&line).ok()?;
            if v["uuid"] != uuid || v["type"] != "user" {
                continue;
            }
            let img = v["message"]["content"]
                .as_array()?
                .iter()
                .filter(|b| b["type"] == "image")
                .nth(index)?;
            let src = &img["source"];
            return Some((
                src["media_type"].as_str()?.to_string(),
                src["data"].as_str()?.to_string(),
            ));
        }
        None
    }

    fn on_entry(
        &mut self,
        line: &Line,
        agent: Option<&str>,
        parent: Option<&str>,
        run: Option<&str>,
        out: &mut Vec<TxEvent>,
    ) {
        let e = &line.entry;
        let uuid = e.uuid.clone().unwrap_or_default();
        let tag = |mut b: Block| {
            b.agent_id = agent.map(str::to_string);
            b.parent_tool_use_id = parent.map(str::to_string);
            b.run_id = run.map(str::to_string);
            b
        };
        match e.entry_type() {
            "system" if agent.is_none() && e.subtype.as_deref() == Some("turn_duration") => {
                self.pending.retain(|_, p| p.agent_id.is_some());
                out.push(TxEvent::TurnEnd {
                    duration_ms: e.duration_ms.map(|d| d.max(0.0) as u64),
                });
            }
            // Newer Claude Code records a local slash command (`/rename`, `/model x`) this way.
            "system" if agent.is_none() && e.subtype.as_deref() == Some("local_command") => {
                let content = decode_str(e.content.as_deref());
                if let Some((model, effort)) = content.as_deref().and_then(local_setting) {
                    out.push(TxEvent::Setting { model, effort });
                }
                if let Some(text) = content.and_then(|c| command_line(&c)) {
                    out.push(TxEvent::UserPrompt {
                        uuid,
                        text,
                        images: 0,
                    });
                }
            }
            "queue-operation" if agent.is_none() => {
                let content =
                    prompt::unwrap_pasted(&decode_str(e.content.as_deref()).unwrap_or_default());
                let before = self.queue.len();
                match e.operation.as_deref() {
                    Some("enqueue") => {
                        self.queue.push(content.clone());
                        out.push(TxEvent::Enqueued { content });
                    }
                    // Absorbed into the running turn (or dropped): by its text; failing that, by
                    // its text with the opening tag's attributes left out (a relayed message is
                    // enqueued with a `hop-chain` attribute that its `remove` does not carry).
                    Some("remove") => self.unqueue(&content),
                    // Taken to run as a turn: the oldest.
                    Some("dequeue") if !self.queue.is_empty() => {
                        self.queue.remove(0);
                    }
                    // `↑` in the TUI takes what can be edited back into the input box, one record
                    // per item (2.1.318); one without its text took all of it.
                    Some("popAll") if e.content.is_some() => {
                        self.unqueue(&content);
                        out.push(TxEvent::Popped { content });
                    }
                    Some("popAll") => self.queue.clear(),
                    _ => {}
                }
                let changed =
                    self.queue.len() != before || e.operation.as_deref() == Some("enqueue");
                if changed {
                    out.push(TxEvent::Queue(self.queue.clone()));
                }
            }
            "attachment" if agent.is_none() => {
                // Read without normalizing: a queued steer stays apart from a prompt.
                let Some(q) = e.queued_command() else {
                    return;
                };
                let Some(text) = q.text().map(|t| prompt::unwrap_pasted(&t)) else {
                    return;
                };
                // A background task that ended while the main line was busy is told this way.
                let kind = q.origin.as_ref().and_then(|o| o.kind.as_deref());
                if q.command_mode.as_deref() == Some("task-notification")
                    || kind == Some("task-notification")
                {
                    if let Some((id, status)) = task_notification(&text)
                        && let Some(t) = self.tasks.iter_mut().find(|t| t.id == id)
                    {
                        t.status = status;
                    }
                    return;
                }
                if q.command_mode.as_deref().is_some_and(|m| m != "prompt") {
                    return;
                }
                let mut b = Block::new(BlockKind::User, &uuid, 0);
                b.text = Some(text);
                b.queued = true;
                out.push(TxEvent::Block(Box::new(b)));
            }
            "user" | "assistant" => {
                if e.is_meta == Some(true) {
                    return;
                }
                let assistant = e.entry_type() == "assistant";
                let Some(msg) = &e.message else {
                    return;
                };
                if assistant {
                    if e.is_api_error_message == Some(true) {
                        if agent.is_none() {
                            let message = match &msg.content {
                                Some(RawContent::Text(t)) => t.clone(),
                                Some(RawContent::Blocks(bs)) => bs
                                    .iter()
                                    .filter_map(|b| b.text_str())
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                None => String::new(),
                            };
                            out.push(TxEvent::ApiError {
                                kind: e.error.clone(),
                                message,
                            });
                        }
                    } else if let (Some(id), Some(u)) = (&msg.id, &msg.usage)
                        && self.usage_seen.insert(id.clone())
                    {
                        let n = |v: Option<f64>| v.unwrap_or(0.0);
                        let cc_1h = u
                            .cache_creation
                            .as_ref()
                            .and_then(|c| c.ephemeral_1h_input_tokens);
                        out.push(TxEvent::Usage {
                            main: agent.is_none(),
                            model: msg.model.clone(),
                            effort: e.effort.clone(),
                            usage: ::transcript::pricing::Usage::from_tokens(
                                n(u.input_tokens),
                                n(u.output_tokens),
                                n(u.cache_read_input_tokens),
                                n(u.cache_creation_input_tokens),
                                n(cc_1h),
                                u.speed.as_deref() == Some("fast"),
                            ),
                            logged_cost: e.cost_usd,
                        });
                    }
                }
                match &msg.content {
                    Some(RawContent::Text(s)) if !assistant => {
                        if prompt::is_command_record(s) {
                            if agent.is_none()
                                && let Some((model, effort)) = local_setting(s)
                            {
                                out.push(TxEvent::Setting { model, effort });
                            }
                            if agent.is_none()
                                && let Some(text) = command_line(s).or_else(|| bash_line(s))
                            {
                                out.push(TxEvent::UserPrompt {
                                    uuid,
                                    text,
                                    images: 0,
                                });
                            }
                            return;
                        }
                        let s = &prompt::unwrap_pasted(s);
                        if agent.is_none()
                            && let Some((id, status)) = task_notification(s)
                            && let Some(t) = self.tasks.iter_mut().find(|t| t.id == id)
                        {
                            t.status = status;
                        }
                        let mut b = Block::new(BlockKind::User, &uuid, 0);
                        b.text = Some(s.clone());
                        // Claude Code's own interruption notes are not prompts and open no turn.
                        if agent.is_none() && !prompt::is_interruption(s) {
                            // A new prompt starts a new turn: its last text is not the previous one's.
                            self.last_text = None;
                            out.push(TxEvent::UserPrompt {
                                uuid: uuid.clone(),
                                text: s.clone(),
                                images: 0,
                            });
                        }
                        out.push(TxEvent::Block(Box::new(tag(b))));
                    }
                    Some(RawContent::Text(s)) => {
                        let mut b = Block::new(BlockKind::Text, &uuid, 0);
                        b.text = Some(s.clone());
                        if agent.is_none() {
                            self.last_text = Some(s.clone());
                        }
                        out.push(TxEvent::Block(Box::new(tag(b))));
                    }
                    Some(RawContent::Blocks(blocks)) => {
                        // A Workflow tool result names its run.
                        if agent.is_none()
                            && let Some(run_id) = e.tool_use_result_fields().and_then(|r| r.run_id)
                            && let Some(tid) = blocks
                                .iter()
                                .find(|b| b.block_type() == "tool_result")
                                .and_then(|b| b.tool_use_id.clone())
                        {
                            self.run_tools.entry(run_id).or_insert(tid);
                        }
                        // A prompt with images: image blocks and its text.
                        let image_prompt = !assistant
                            && agent.is_none()
                            && blocks.iter().any(|b| b.block_type() == "image")
                            && !blocks.iter().any(|b| b.block_type() == "tool_result");
                        if image_prompt {
                            let text: Vec<String> = blocks
                                .iter()
                                .filter_map(|b| b.text_str())
                                .map(|t| prompt::unwrap_pasted(&t))
                                .collect();
                            self.last_text = None;
                            out.push(TxEvent::UserPrompt {
                                uuid: uuid.clone(),
                                text: text.join("\n"),
                                images: blocks.iter().filter(|b| b.block_type() == "image").count(),
                            });
                        }
                        let mut images: Vec<String> = if image_prompt {
                            blocks
                                .iter()
                                .filter(|b| b.block_type() == "image")
                                .map(image_type)
                                .collect()
                        } else {
                            Vec::new()
                        };
                        for (i, rb) in blocks.iter().enumerate() {
                            if let Some(mut b) = self.block_of(rb, &uuid, i, assistant, agent) {
                                if b.kind == BlockKind::User && !images.is_empty() {
                                    b.images = std::mem::take(&mut images);
                                }
                                out.push(TxEvent::Block(Box::new(tag(b))));
                            }
                        }
                        // Images without any text: a user block of their own.
                        if !images.is_empty() {
                            let mut b = Block::new(BlockKind::User, &uuid, 0);
                            b.text = Some(String::new());
                            b.images = images;
                            out.push(TxEvent::Block(Box::new(tag(b))));
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn block_of(
        &mut self,
        rb: &RawBlock,
        uuid: &str,
        i: usize,
        assistant: bool,
        agent: Option<&str>,
    ) -> Option<Block> {
        match rb.block_type() {
            "text" => {
                let t = rb.text_str()?;
                let t = if assistant {
                    t
                } else {
                    prompt::unwrap_pasted(&t)
                };
                if assistant && agent.is_none() {
                    self.last_text = Some(t.clone());
                }
                let mut b = Block::new(
                    if assistant {
                        BlockKind::Text
                    } else {
                        BlockKind::User
                    },
                    uuid,
                    i,
                );
                b.text = Some(cap(t));
                Some(b)
            }
            // Narration (`RawBlock::is_narration`) is text in the TUI, so it is a text block here too.
            "thinking"
                if assistant
                    && rb.is_narration()
                    && rb.thinking_str().is_some_and(|t| !t.trim().is_empty()) =>
            {
                let t = rb.thinking_str().unwrap_or_default();
                if agent.is_none() {
                    self.last_text = Some(t.clone());
                }
                let mut b = Block::new(BlockKind::Text, uuid, i);
                b.text = Some(cap(t));
                Some(b)
            }
            "thinking" if assistant => {
                let mut b = Block::new(BlockKind::Thinking, uuid, i);
                b.text = Some(cap(rb.thinking_str().unwrap_or_default()));
                Some(b)
            }
            "tool_use" if assistant => {
                let id = rb.id.clone().unwrap_or_default();
                let name = rb.name.clone().unwrap_or_default();
                let input = rb
                    .input
                    .as_deref()
                    .and_then(|r| serde_json::from_str::<Value>(r.get()).ok())
                    .unwrap_or(Value::Null);
                if name == "SendFeedback" && agent.is_none() {
                    self.last_feedback = Some((id.clone(), input.clone()));
                }
                self.seq += 1;
                self.pending.insert(
                    id.clone(),
                    PendingTool {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        agent_id: agent.map(str::to_string),
                        seq: self.seq,
                    },
                );
                let mut b = Block::new(BlockKind::ToolUse, uuid, i);
                b.id = Some(id);
                b.name = Some(name);
                b.input = Some(input);
                Some(b)
            }
            "tool_result" if !assistant => {
                let tid = rb.tool_use_id.clone().unwrap_or_default();
                let tool = self.pending.remove(&tid);
                let mut b = Block::new(BlockKind::ToolResult, uuid, i);
                let text = nodes::result_content(rb.content.as_deref()).0;
                // A task stopped with TaskStop gets no notification of its own (a shell's never
                // does): its result is the only record that the task ended.
                if agent.is_none()
                    && rb.is_error != Some(true)
                    && let Some(t) = tool.as_ref().filter(|t| t.name == "TaskStop")
                    && let Some(id) = ["task_id", "shell_id"]
                        .iter()
                        .find_map(|k| t.input.get(*k).and_then(Value::as_str))
                {
                    let id = id.to_string();
                    self.mark_stopped(&id);
                }
                if agent.is_none()
                    && let Some((kind, id)) = subagent::background_launch(&text)
                    && let Some(kind) = TaskKind::from_name(kind)
                    && !self.tasks.iter().any(|t| t.id == id)
                {
                    let input = tool.as_ref().map(|t| &t.input);
                    let field = |k: &str| {
                        input
                            .and_then(|i| i.get(k))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    };
                    let summary = text
                        .lines()
                        .find_map(|l| l.strip_prefix("Summary: "))
                        .map(str::to_string);
                    self.tasks.push(BgTask {
                        id,
                        kind,
                        tool_use_id: tid.clone(),
                        description: field("description").or(summary),
                        command: field("command"),
                        status: "running".into(),
                    });
                }
                if let Some(path) = persisted_path(&text, &self.session_dir()) {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    b.persisted = Some(Persisted {
                        path: path.display().to_string(),
                        size,
                    });
                    self.persisted.insert(tid.clone(), path);
                }
                b.tool_use_id = Some(tid);
                b.content = Some(cap(text));
                b.is_error = Some(rb.is_error.unwrap_or(false));
                Some(b)
            }
            _ => None,
        }
    }
}

/// Blocks of the main file on the Main Line written at or after byte `since`, and the byte offset
/// to pass next time (for `get_messages`: offsets stay valid when the Main Line changes). The
/// file is tailed from the start exactly as live, so the result is what a caller following the
/// events holds: blocks dropped at each `branch`, nothing else.
pub fn main_blocks(main: &Path, since: u64) -> (Vec<Block>, u64) {
    let mut t = Tailer::new(main);
    t.main_only = true;
    let mut v: Vec<Block> = Vec::new();
    loop {
        for ev in t.poll() {
            match ev {
                TxEvent::Block(b) => v.push(*b),
                TxEvent::Branch { at_uuid } => {
                    let keep = at_uuid
                        .and_then(|a| v.iter().rposition(|b| b.uuid == a))
                        .map_or(0, |i| i + 1);
                    v.truncate(keep);
                }
                _ => {}
            }
        }
        if !t.behind() {
            break;
        }
    }
    let offsets = std::mem::take(&mut t.offsets);
    v.retain(|b| offsets.get(&b.uuid).is_some_and(|&o| o >= since));
    (v, t.offset)
}

/// uuids of the entries of `main` on an abandoned Branch, with the Main Line going through `leaf`
/// (a prompt that forked, at byte `upto`; later entries are ignored; Claude Code's own leaf
/// marker is written late) down to its newest descendant (children written before their parent
/// stay on it).
/// Entries in the Main Line's tree or under a real root (no parent) but not on the Main Line are
/// abandoned; entries cut off by a dangling parent are a fragment, not a Branch, and are kept.
/// The Main Line itself follows the transcript crate's rules (merged fragments, absorbed tool results, compact
/// boundaries). `None` if unreadable.
pub fn off_main_uuids(main: &Path, leaf: &str, upto: u64) -> Option<HashSet<String>> {
    let mut chunk = ::transcript::parse::read_entries(main, 0).ok()?;
    chunk.records.retain(|r| r.offset <= upto);
    let real_root: HashSet<String> = chunk
        .records
        .iter()
        .filter(|r| r.entry.parent_uuid.is_none() && r.entry.logical_parent_uuid.is_none())
        .filter_map(|r| r.entry.uuid.clone())
        .collect();
    let files = [LoadedFile {
        path: main.to_path_buf(),
        role: FileRole::Main,
        agent_id: None,
        records: chunk.records,
    }];
    let mut s = skeleton::build(&files);
    // Root of every node, memoized (parents are cycle-free).
    let mut root: Vec<Option<usize>> = vec![None; s.nodes.len()];
    let mut root_of = |i: usize| {
        let mut path = vec![];
        let mut v = i;
        let r = loop {
            if let Some(r) = root[v] {
                break r;
            }
            match s.nodes[v].parent {
                Some(p) => {
                    path.push(v);
                    v = p;
                }
                None => break v,
            }
        };
        root[v] = Some(r);
        for u in path {
            root[u] = Some(r);
        }
        r
    };
    let leaf = tree::descend(&s, *s.by_uuid.get(leaf)?);
    s.main_path = tree::root_path(&s, leaf);
    let on_main = tree::main_set(&s);
    let main_root = root_of(*s.main_path.first()?);
    Some(
        (0..s.nodes.len())
            .filter(|&i| {
                let r = root_of(i);
                (r == main_root || real_root.contains(&s.nodes[r].uuid))
                    && !on_main.contains(&s.nodes[i].uuid)
            })
            .map(|i| s.nodes[i].uuid.clone())
            .collect(),
    )
}

/// The media type of an image block (`image/png` when not recorded).
fn image_type(b: &RawBlock) -> String {
    b.source
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s.get()).ok())
        .and_then(|v| v["media_type"].as_str().map(str::to_string))
        .unwrap_or_else(|| "image/png".into())
}

/// The prompt text of a main-file user prompt entry.
fn prompt_text(l: &Line) -> Option<String> {
    let e = &l.entry;
    if e.entry_type() != "user" || e.is_meta == Some(true) {
        return None;
    }
    match e.message.as_ref()?.content.as_ref()? {
        RawContent::Text(t) if !prompt::is_command_record(t) && !prompt::is_interruption(t) => {
            Some(prompt::unwrap_pasted(t))
        }
        _ => None,
    }
}

/// The file a `<persisted-output>` preview points to, only when the preview names this Session's
/// own `tool-results/` (a tool's own output could imitate the preview).
fn persisted_path(text: &str, session_dir: &Path) -> Option<PathBuf> {
    let claimed = Path::new(nodes::persisted_output_path(text)?);
    let name = content::persisted_basename(claimed.to_str()?)?;
    let p = content::persisted_path(session_dir, &name)?;
    (claimed.parent() == p.parent()).then_some(p)
}

/// A regular text file read with the block cap (read-only; no symlinks, nothing that blocks).
fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let mut buf = Vec::new();
    f.take(MAX_BLOCK_TEXT as u64 + 4)
        .read_to_end(&mut buf)
        .ok()?;
    Some(cap(String::from_utf8_lossy(&buf).into_owned()))
}

fn read_meta_tool_use_id(dir: &Path, agent_id: &str) -> Option<String> {
    let bytes = std::fs::read(layout::agent_meta_path(dir, agent_id)).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("toolUseId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One complete transcript line (queued commands not rewritten: a steer stays apart).
struct Line {
    entry: RawEntry,
    /// Byte offset of the line in its file.
    offset: u64,
}

struct Chunk {
    lines: Vec<Line>,
    consumed_to: u64,
    /// The read stopped at the size cap: more complete lines may follow.
    capped: bool,
}

/// A slash command's record (`<command-name>/x</command-name>…<command-args>a</command-args>`) as
/// it was typed: `/x a`. Its output and `!` shell records are not commands.
/// A `!` command's record (`<bash-input>ls</bash-input>`) as `! ls`, as the TUI writes it.
fn bash_line(s: &str) -> Option<String> {
    let t = s.trim_start();
    if !t.starts_with("<bash-input>") {
        return None;
    }
    Some(format!("! {}", prompt::tag(t, "bash-input")?.trim()))
}

fn command_line(s: &str) -> Option<String> {
    let t = s.trim_start();
    if !t.starts_with("<command-name>") && !t.starts_with("<command-message>") {
        return None;
    }
    let name = prompt::tag(t, "command-name")?;
    let args = prompt::tag(t, "command-args").unwrap_or_default();
    let line = format!("/{} {}", name.trim().trim_start_matches('/'), args.trim());
    Some(line.trim_end().to_string())
}

/// Reads the complete lines of `path` from byte `from` (read-only; a partial last line is left
/// for the next read; lines that are not JSON objects are skipped), at most [`MAX_READ`] bytes,
/// without rewriting `queued_command` attachments (steer needs them apart from prompts).
fn read_lines(path: &Path, from: u64) -> std::io::Result<Chunk> {
    let opts = ReadOptions {
        max_bytes: Some(MAX_READ),
        normalize: false,
    };
    let c = read_entries_with(path, from, opts)?;
    Ok(Chunk {
        lines: c
            .records
            .into_iter()
            .map(|r| Line {
                entry: r.entry,
                offset: r.offset,
            })
            .collect(),
        // A file that shrank: continue from its new end.
        consumed_to: c.consumed_to.min(c.file_len),
        capped: c.capped,
    })
}

/// The uuids of every entry in the transcript `path` (the main file only).
pub fn entry_uuids(path: &Path) -> HashSet<String> {
    let mut uuids = HashSet::new();
    let mut from = 0;
    while let Ok(c) = read_lines(path, from) {
        uuids.extend(c.lines.into_iter().filter_map(|l| l.entry.uuid));
        if !c.capped || c.consumed_to <= from {
            break;
        }
        from = c.consumed_to;
    }
    uuids
}

/// A `<task-notification>` that ended a task: `(task id, status)`.
pub fn task_notification(text: &str) -> Option<(String, String)> {
    let n = prompt::task_notification(text)?;
    Some((n.task_id?, n.status?))
}

fn cap(mut s: String) -> String {
    if s.len() > MAX_BLOCK_TEXT {
        let mut end = MAX_BLOCK_TEXT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str("…[truncated]");
    }
    s
}

/// A queued item's text with the attributes of a leading tag on its own line left out:
/// `<cross-session-message from="…" hop-chain="…">` → `<cross-session-message>`.
fn queue_key(text: &str) -> String {
    let t = text.trim();
    if let Some((first, rest)) = t.split_once('\n')
        && let Some(open) = first.strip_prefix('<')
        && first.ends_with('>')
    {
        let name: String = open
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
            .collect();
        if !name.is_empty() {
            return format!("<{name}>\n{rest}");
        }
    }
    t.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/transcripts")
            .join(name)
    }

    fn blocks(evs: &[TxEvent]) -> Vec<&Block> {
        evs.iter()
            .filter_map(|e| {
                if let TxEvent::Block(b) = e {
                    Some(b.as_ref())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn reads_prompts_blocks_turn_ends_and_steer() {
        let mut t = Tailer::new(&fixture("steer_ask_permission.jsonl"));
        let evs = t.poll();
        let prompts: Vec<_> = evs
            .iter()
            .filter_map(|e| {
                if let TxEvent::UserPrompt { text, .. } = e {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            prompts[0],
            "Reply with exactly the word PONG and nothing else."
        );
        let turn_ends = evs
            .iter()
            .filter(|e| matches!(e, TxEvent::TurnEnd { .. }))
            .count();
        assert!(turn_ends >= 5, "{turn_ends}");
        let bs = blocks(&evs);
        assert!(
            bs.iter()
                .any(|b| b.kind == BlockKind::Text && b.text.as_deref() == Some("PONG"))
        );
        let ask = bs
            .iter()
            .find(|b| b.name.as_deref() == Some("AskUserQuestion"))
            .unwrap();
        assert!(ask.input.as_ref().unwrap()["questions"].is_array());
        assert!(bs.iter().any(|b| {
            b.kind == BlockKind::ToolResult
                && b.content
                    .as_deref()
                    .unwrap_or("")
                    .starts_with("Your questions have been answered")
        }));
        // steer: enqueue marker and the queued_command delivered as a queued user block
        assert!(
            evs.iter().any(
                |e| matches!(e, TxEvent::Enqueued { content } if content.starts_with("STEER:"))
            )
        );
        assert!(
            bs.iter()
                .any(|b| b.queued && b.text.as_deref().unwrap_or("").starts_with("STEER:"))
        );
        // every tool_use got its result, so nothing is pending at the end
        assert!(t.pending().is_empty(), "{:?}", t.pending());
        assert!(t.last_text().is_some());
    }

    #[test]
    fn subagent_blocks_are_tagged_and_command_records_skipped() {
        let mut t = Tailer::new(&fixture("subagent_deny.jsonl"));
        let evs = t.poll();
        let bs = blocks(&evs);
        assert!(
            !bs.iter()
                .any(|b| b.text.as_deref().unwrap_or("").contains("<command-name>"))
        );
        let sub: Vec<_> = bs.iter().filter(|b| b.agent_id.is_some()).collect();
        assert!(!sub.is_empty());
        assert!(
            sub.iter()
                .all(|b| b.agent_id.as_deref() == Some("a1f4e2d8aa4a5bb8d")
                    && b.parent_tool_use_id.as_deref()
                        == Some("call_00_ET_Y0xzg91x84thLmZ5PEby7243"))
        );
        assert!(sub.iter().any(|b| b.name.as_deref() == Some("Bash")));
        let agent_call = bs
            .iter()
            .find(|b| b.name.as_deref() == Some("Agent"))
            .unwrap();
        assert_eq!(
            agent_call.id.as_deref(),
            Some("call_00_ET_Y0xzg91x84thLmZ5PEby7243")
        );
        // the denied rm still has an (error) tool_result, so nothing is left pending
        assert!(t.pending().is_empty(), "{:?}", t.pending());
    }

    #[test]
    fn incremental_reads_and_pending_tools() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let src = std::fs::read_to_string(fixture("steer_ask_permission.jsonl")).unwrap();
        let lines: Vec<&str> = src.lines().collect();
        let cut = lines
            .iter()
            .position(|l| l.contains("\"name\": \"AskUserQuestion\""))
            .unwrap();
        // write up to and including the AskUserQuestion tool_use, plus half of the next line
        let mut head = lines[..=cut].join("\n");
        head.push('\n');
        head.push_str(&lines[cut + 1][..10]);
        std::fs::write(&main, &head).unwrap();
        let mut t = Tailer::new(&main);
        assert!(!t.poll().is_empty(), "first poll returns the history");
        assert_eq!(
            t.pending_named("AskUserQuestion").map(|p| p.name.as_str()),
            Some("AskUserQuestion")
        );
        assert!(
            t.poll().is_empty(),
            "catch-up is silent and the partial line waits"
        );
        let mut rest = lines[cut + 1..].join("\n");
        rest.push('\n');
        std::fs::write(
            &main,
            format!("{}{}", lines[..=cut].join("\n") + "\n", rest),
        )
        .unwrap();
        let evs = t.poll();
        assert!(!evs.is_empty());
        assert!(t.pending_named("AskUserQuestion").is_none());
    }

    #[test]
    fn model_and_effort_outputs_say_what_was_set() {
        assert_eq!(
            local_setting(
                "<local-command-stdout>Set model to `Sonnet 4.5` and saved as your default for new sessions\n         ANTHROPIC_MODEL is set</local-command-stdout>"
            ),
            Some((Some("Sonnet 4.5".into()), None))
        );
        assert_eq!(
            local_setting(
                "Set model to Opus 4.7 (default) and saved as your default for new sessions"
            ),
            Some((Some("Opus 4.7".into()), None))
        );
        assert_eq!(
            local_setting("Kept model as `Opus 4.7`"),
            Some((Some("Opus 4.7".into()), None))
        );
        assert_eq!(
            local_setting(
                "<local-command-stdout>Set effort level to low (saved as your default for new sessions): Quick</local-command-stdout>"
            ),
            Some((None, Some("low".into())))
        );
        assert_eq!(
            local_setting("<local-command-stdout>Cancelled</local-command-stdout>"),
            None
        );
        // Both record shapes: a `user` entry (2.1.316) and a `system` / `local_command` one.
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let user = json!({"type": "user", "uuid": "o1", "message": {"role": "user",
            "content": "<local-command-stdout>Set effort level to low (saved as your default for new sessions): Quick</local-command-stdout>"}});
        let system = json!({"type": "system", "subtype": "local_command", "uuid": "o2",
            "content": "<local-command-stdout>Set model to `Fable 5.1` and saved as your default for new sessions</local-command-stdout>"});
        let reply = json!({"type": "assistant", "uuid": "a1", "effort": "high",
            "message": {"id": "m1", "model": "claude-fable-5-1", "role": "assistant",
                "content": [{"type": "text", "text": "ok"}], "usage": {"input_tokens": 1, "output_tokens": 1}}});
        write_lines(&main, &[user, system, reply]);
        let evs = Tailer::new(&main).poll();
        let got: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                TxEvent::Setting { model, effort } => Some((model.clone(), effort.clone())),
                TxEvent::Usage { model, effort, .. } => Some((model.clone(), effort.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (None, Some("low".into())),
                (Some("Fable 5.1".into()), None),
                (Some("claude-fable-5-1".into()), Some("high".into())),
            ]
        );
    }

    #[test]
    fn slash_command_records_confirm_as_typed_without_blocks() {
        // Claude Code 2.1.318: a local command as a `system` entry, a prompt-type one as a `user` entry.
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let local = json!({"type": "system", "subtype": "local_command", "uuid": "c1",
            "content": "<command-name>/rename</command-name>\n            <command-message>rename</command-message>\n            <command-args>probe-a</command-args>"});
        let output = json!({"type": "system", "subtype": "local_command", "uuid": "c2",
            "content": "<local-command-stdout>Session renamed to: probe-a</local-command-stdout>"});
        let skill = json!({"type": "user", "uuid": "c3", "message": {"role": "user",
            "content": "<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args></command-args>"}});
        write_lines(&main, &[local, output, skill]);
        let evs = Tailer::new(&main).poll();
        let prompts: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                TxEvent::UserPrompt { uuid, text, .. } => Some((uuid.as_str(), text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(prompts, vec![("c1", "/rename probe-a"), ("c3", "/review")]);
        assert!(blocks(&evs).is_empty());
    }

    #[test]
    fn queue_follows_the_queue_operations() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let op = |op: &str, content: Option<&str>| {
            let mut v = json!({"type": "queue-operation", "operation": op});
            if let Some(c) = content {
                v["content"] = json!(c);
            }
            v
        };
        write_lines(
            &main,
            &[
                op("enqueue", Some("a")),
                op("enqueue", Some("b")),
                op("enqueue", Some("c")),
            ],
        );
        let mut t = Tailer::new(&main);
        let queues = |evs: Vec<TxEvent>| -> Vec<Vec<String>> {
            evs.into_iter()
                .filter_map(|e| {
                    if let TxEvent::Queue(q) = e {
                        Some(q)
                    } else {
                        None
                    }
                })
                .collect()
        };
        assert_eq!(queues(t.poll()).last().unwrap(), &vec!["a", "b", "c"]);
        let more = [
            op("remove", Some("b")),
            op("dequeue", None),
            op("popAll", None),
        ];
        let text: String = more.iter().map(|v| format!("{v}\n")).collect();
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&main)
            .unwrap();
        std::io::Write::write_all(&mut f, text.as_bytes()).unwrap();
        let q = queues(t.poll());
        assert_eq!(q, vec![vec!["a", "c"], vec!["c"], vec![]]);
        assert!(t.queue().is_empty());
        // A relayed message is enqueued with a `hop-chain` attribute its `remove` lacks.
        let relayed = |attrs: &str| {
            format!(
                "<cross-session-message from=\"uds:/tmp/a.sock\"{attrs} from-name=\"c6\">\nhello\n</cross-session-message>"
            )
        };
        let more = [
            op("enqueue", Some(&relayed(" hop-chain=\"8a7e\""))),
            op("remove", Some(&relayed(""))),
        ];
        let text: String = more.iter().map(|v| format!("{v}\n")).collect();
        std::io::Write::write_all(&mut f, text.as_bytes()).unwrap();
        assert_eq!(queues(t.poll()).last().unwrap(), &Vec::<String>::new());
        // `↑` records each item it takes back; a message from another Session stays queued.
        let more = [
            op("enqueue", Some("d")),
            op("enqueue", Some(&relayed(""))),
            op("enqueue", Some("e")),
            op("popAll", Some("d")),
            op("popAll", Some("e")),
        ];
        let text: String = more.iter().map(|v| format!("{v}\n")).collect();
        std::io::Write::write_all(&mut f, text.as_bytes()).unwrap();
        let evs = t.poll();
        let popped: Vec<&str> = evs
            .iter()
            .filter_map(|e| match e {
                TxEvent::Popped { content } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(popped, ["d", "e"]);
        assert_eq!(t.queue(), [relayed("")]);
    }

    /// A main file that shrank was replaced: nothing read from the old one is kept (queue,
    /// pending tools, entry offsets), as if tailing the new file from the start.
    #[test]
    fn a_replaced_main_file_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let enqueue =
            |c: &str| json!({"type": "queue-operation", "operation": "enqueue", "content": c});
        let tool_use = json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1",
            "message": {"id": "m-a1", "role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_old", "name": "Bash", "input": {"command": "ls"}}]}});
        write_lines(
            &main,
            &[
                prompt("u1", None, "a long first prompt from the old file", 1),
                tool_use,
                enqueue("old one"),
                enqueue("old two"),
            ],
        );
        // A Subagent Run file next to it, and a reply whose usage the new file has again.
        let agents = subagents_dir(&main);
        std::fs::create_dir_all(&agents).unwrap();
        write_lines(
            &agents.join("agent-ag1.jsonl"),
            &[prompt("x1", None, "sub prompt", 1)],
        );
        let mut billed = reply("r1", "u1", "done", 2);
        billed["message"]["usage"] = json!({"input_tokens": 10, "output_tokens": 3});
        let mut old_lines = std::fs::read_to_string(&main).unwrap();
        old_lines.push_str(&format!("{billed}\n"));
        std::fs::write(&main, old_lines).unwrap();
        let mut t = Tailer::new(&main);
        t.keep_offsets();
        let first = t.poll();
        assert!(
            blocks(&first)
                .iter()
                .any(|b| b.agent_id.as_deref() == Some("ag1"))
        );
        assert!(first.iter().any(|e| matches!(e, TxEvent::Usage { .. })));
        assert_eq!(t.queue(), ["old one", "old two"]);
        assert!(t.pending_by_id("toolu_old").is_some());
        write_lines(
            &main,
            &[prompt("n1", None, "new", 1), enqueue("new"), billed],
        );
        let evs = t.poll();
        // Only the main file starts over: the Subagent Run and the usage are not reported again.
        assert!(
            !blocks(&evs).iter().any(|b| b.agent_id.is_some()),
            "{evs:?}"
        );
        assert!(
            !evs.iter().any(|e| matches!(e, TxEvent::Usage { .. })),
            "{evs:?}"
        );
        assert!(blocks(&evs).iter().any(|b| b.uuid == "n1"));
        assert_eq!(t.queue(), ["new"]);
        assert!(t.pending().is_empty());
        assert_eq!(t.offset_of("u1"), None);
        assert_eq!(t.offset_of("n1"), Some(0));
        assert_eq!(t.read_to(), std::fs::metadata(&main).unwrap().len());
    }

    #[test]
    fn interruption_note_is_a_block_but_not_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let line = r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"[Request interrupted by user]"}}"#;
        std::fs::write(&main, format!("{line}\n")).unwrap();
        let evs = Tailer::new(&main).poll();
        assert!(!evs.iter().any(|e| matches!(e, TxEvent::UserPrompt { .. })));
        assert_eq!(blocks(&evs).len(), 1);
    }

    #[test]
    fn wrapped_paste_with_an_image_reads_as_sent() {
        // A long paste after an image, as a real account records it (2.1.317).
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let text = "[Image #1]\n\n<pasted_content id=\"1b44\">\nholder_id: m1\n\n现在有问题\n</pasted_content id=\"1b44\">\n";
        let line = json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": [
            {"type": "text", "text": text},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}},
        ]}});
        let note = json!({"type": "user", "uuid": "u2", "isMeta": true, "message": {"role": "user", "content": [
            {"type": "text", "text": "[Image: source: /tmp/x.png]"},
        ]}});
        write_lines(&main, &[line, note]);
        let t = Tailer::new(&main);
        let evs = {
            let mut t = Tailer::new(&main);
            t.poll()
        };
        // The image note is not a block; the prompt's block names its image, fetched by uuid.
        assert_eq!(blocks(&evs).len(), 1, "{evs:?}");
        assert_eq!(blocks(&evs)[0].images, vec!["image/png".to_string()]);
        assert_eq!(
            t.image("u1", 0),
            Some(("image/png".into(), "iVBORw0KGgo=".into()))
        );
        assert_eq!(t.image("u1", 1), None);
        assert_eq!(t.image("u2", 0), None);
        let prompt = evs.iter().find_map(|e| match e {
            TxEvent::UserPrompt { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(
            prompt.as_deref().map(str::trim),
            Some("[Image #1]\n\nholder_id: m1\n\n现在有问题")
        );
        assert_eq!(
            blocks(&evs)[0].text.as_deref().map(str::trim),
            Some("[Image #1]\n\nholder_id: m1\n\n现在有问题")
        );
    }

    #[test]
    fn main_blocks_reads_whole_file() {
        let path = fixture("steer_ask_permission.jsonl");
        let (bs, cursor) = main_blocks(&path, 0);
        // (The fixture is trimmed: its chain is cut in places, so only counts are checked.)
        assert!(bs.len() > 20);
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len());
        assert!(main_blocks(&path, cursor).0.is_empty());
    }

    /// Writes `lines` (one JSON object each) to `path`.
    fn write_lines(path: &Path, lines: &[Value]) {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(path, text).unwrap();
    }

    fn prompt(uuid: &str, parent: Option<&str>, text: &str, ts: u32) -> Value {
        json!({"type": "user", "uuid": uuid, "parentUuid": parent,
            "timestamp": format!("2026-11-03T00:00:{ts:02}Z"),
            "message": {"role": "user", "content": text}})
    }

    fn reply(uuid: &str, parent: &str, text: &str, ts: u32) -> Value {
        json!({"type": "assistant", "uuid": uuid, "parentUuid": parent,
            "timestamp": format!("2026-11-03T00:00:{ts:02}Z"),
            "message": {"id": format!("m-{uuid}"), "role": "assistant",
                "content": [{"type": "text", "text": text}]}})
    }

    fn last_prompt(leaf: &str) -> Value {
        json!({"type": "last-prompt", "leafUuid": leaf})
    }

    fn attachment(uuid: &str, parent: &str) -> Value {
        json!({"type": "attachment", "uuid": uuid, "parentUuid": parent,
            "attachment": {"type": "todo_reminder"}})
    }

    fn interruption(uuid: &str, parent: &str) -> Value {
        json!({"type": "user", "uuid": uuid, "parentUuid": parent,
            "message": {"role": "user", "content": "[Request interrupted by user]"}})
    }

    /// Tails `lines`, then `more` appended; returns the events of the second poll.
    fn tail_then(main: &Path, lines: &[Value], more: &[Value]) -> (Tailer, Vec<TxEvent>) {
        write_lines(main, lines);
        let mut t = Tailer::new(main);
        assert!(!t.poll().iter().any(|e| matches!(e, TxEvent::Branch { .. })));
        let all: Vec<Value> = lines.iter().chain(more).cloned().collect();
        write_lines(main, &all);
        let evs = t.poll();
        (t, evs)
    }

    fn kinds(evs: &[TxEvent]) -> Vec<String> {
        evs.iter()
            .filter_map(|e| match e {
                TxEvent::Branch { at_uuid } => {
                    Some(format!("branch:{}", at_uuid.as_deref().unwrap_or("-")))
                }
                TxEvent::Block(b) => Some(format!("block:{}", b.uuid)),
                _ => None,
            })
            .collect()
    }

    fn history() -> Vec<Value> {
        vec![
            prompt("u1", None, "first", 1),
            reply("a1", "u1", "one", 2),
            last_prompt("a1"),
            prompt("u2", Some("a1"), "second", 3),
            reply("a2", "u2", "two", 4),
            last_prompt("a2"),
        ]
    }

    /// Moved to the background: the new transcript starts with a copy of the old one, written
    /// after ccrpc switched to it (and possibly in several appends). The copy gives no blocks,
    /// usage, turn ends or prompts; what follows it does.
    #[test]
    fn copied_entries_of_a_continued_session_produce_no_events() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old.jsonl"), dir.path().join("new.jsonl"));
        let usage = json!({"input_tokens": 10, "output_tokens": 5});
        let replied = |uuid: &str, parent: &str, id: &str| {
            json!({"type": "assistant", "uuid": uuid, "parentUuid": parent,
                "message": {"id": id, "model": "claude-sonnet-4-5", "role": "assistant", "usage": usage,
                    "content": [{"type": "text", "text": "old"}]}})
        };
        let copy = vec![
            prompt("u1", None, "first", 1),
            replied("a1", "u1", "m1"),
            json!({"type": "system", "subtype": "turn_duration", "uuid": "d1", "parentUuid": "a1", "durationMs": 10}),
            interruption("i1", "d1"),
            json!({"type": "cost-state", "totalCostUSD": 1.0}),
        ];
        write_lines(&old, &copy);
        let mut t = Tailer::new(&new);
        t.copied_from(entry_uuids(&old));
        assert!(t.poll().is_empty(), "no file yet");
        // The copy arrives in two appends, a line without uuid among it.
        write_lines(&new, &copy[..2]);
        assert!(t.poll().is_empty());
        write_lines(&new, &copy);
        assert!(t.poll().is_empty());
        let mut all = copy.clone();
        all.extend([
            prompt("u2", Some("i1"), "second", 5),
            replied("a2", "u2", "m2"),
            json!({"type": "system", "subtype": "turn_duration", "uuid": "d2", "parentUuid": "a2", "durationMs": 20}),
        ]);
        write_lines(&new, &all);
        let evs = t.poll();
        assert_eq!(kinds(&evs), ["block:u2", "block:a2"]);
        let usages = evs
            .iter()
            .filter(|e| matches!(e, TxEvent::Usage { .. }))
            .count();
        assert_eq!(usages, 1);
        assert!(evs.iter().any(|e| matches!(
            e,
            TxEvent::TurnEnd {
                duration_ms: Some(20)
            }
        )));
        assert!(
            evs.iter()
                .any(|e| matches!(e, TxEvent::UserPrompt { text, .. } if text == "second"))
        );
        assert!(
            !evs.iter()
                .any(|e| matches!(e, TxEvent::UserPrompt { text, .. } if text == "first"))
        );
    }

    #[test]
    fn rewind_reports_the_branch_before_its_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        // Rewind to after "one" and ask something else; the leaf marker comes later.
        let (_, evs) = tail_then(
            &main,
            &history(),
            &[
                prompt("u3", Some("a1"), "other", 5),
                reply("a3", "u3", "three", 6),
            ],
        );
        assert_eq!(kinds(&evs), ["branch:a1", "block:u3", "block:a3"]);
        let texts: Vec<_> = main_blocks(&main, 0)
            .0
            .into_iter()
            .filter_map(|b| b.text)
            .collect();
        assert_eq!(texts, ["first", "one", "other", "three"]);
    }

    #[test]
    fn a_branch_starting_with_an_attachment_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let (_, evs) = tail_then(
            &main,
            &history(),
            &[attachment("x3", "a1"), prompt("u3", Some("x3"), "other", 5)],
        );
        assert_eq!(kinds(&evs)[0], "branch:a1");
    }

    #[test]
    fn rewinding_to_the_start_is_a_branch_too() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let (_, evs) = tail_then(&main, &history(), &[prompt("u3", None, "fresh", 5)]);
        assert_eq!(kinds(&evs)[0], "branch:-");
        let texts: Vec<_> = main_blocks(&main, 0)
            .0
            .into_iter()
            .filter_map(|b| b.text)
            .collect();
        assert_eq!(texts, ["fresh"]);
    }

    fn system(uuid: &str, parent: Option<&str>, subtype: &str, logical: Option<&str>) -> Value {
        json!({"type": "system", "subtype": subtype, "uuid": uuid, "parentUuid": parent,
            "logicalParentUuid": logical, "content": ""})
    }

    #[test]
    fn narration_is_a_text_block_and_the_last_text() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let think = |uuid: &str, parent: &str, text: &str, sig: &str| {
            json!({"type": "assistant", "uuid": uuid, "parentUuid": parent, "message": {"id": uuid, "role": "assistant",
                "content": [{"type": "thinking", "thinking": text, "signature": sig}]}})
        };
        write_lines(
            &main,
            &[
                prompt("u1", None, "hi", 1),
                think(
                    "a1",
                    "u1",
                    "private",
                    "CAQSLgoRCBMYAjgBQgh0aGlua2luZxIMAAECAwQFBgcICQoLGgwMDQ4PEBESExQVFhc=",
                ),
                think(
                    "a2",
                    "a1",
                    "Checking the layout.",
                    "CAQSLwoRCBMYAjgBQgluYXJyYXRpb24SDAABAgMEBQYHCAkKCxoMDA0ODxAREhMUFRYX",
                ),
            ],
        );
        let mut t = Tailer::new(&main);
        let blocks: Vec<_> = t
            .poll()
            .into_iter()
            .filter_map(|e| match e {
                TxEvent::Block(b) => Some((b.kind, b.text)),
                _ => None,
            })
            .collect();
        assert!(blocks.contains(&(BlockKind::Thinking, Some("private".into()))));
        assert!(blocks.contains(&(BlockKind::Text, Some("Checking the layout.".into()))));
        assert_eq!(t.last_text(), Some("Checking the layout."));
    }

    #[test]
    fn usage_once_per_message_api_errors_and_turn_duration() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let usage = json!({"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100,
            "cache_creation_input_tokens": 40, "cache_creation": {"ephemeral_1h_input_tokens": 30}});
        let part = |uuid: &str, kind: &str| {
            json!({"type": "assistant", "uuid": uuid, "message": {"id": "msg_1", "model": "claude-sonnet-5-5",
                "role": "assistant", "usage": usage, "content": [{"type": kind, "text": "x", "thinking": ""}]}})
        };
        write_lines(
            &main,
            &[
                prompt("u1", None, "hi", 1),
                part("a1", "thinking"),
                part("a2", "text"),
                json!({"type": "assistant", "uuid": "e1", "isApiErrorMessage": true, "error": "rate_limit",
                    "message": {"id": "e", "model": "<synthetic>", "role": "assistant",
                        "content": [{"type": "text", "text": "API Error: 429"}]}}),
                json!({"type": "system", "subtype": "turn_duration", "uuid": "d1", "durationMs": 4200}),
            ],
        );
        let evs = Tailer::new(&main).poll();
        let usages: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                TxEvent::Usage { usage, .. } => Some(*usage),
                _ => None,
            })
            .collect();
        assert_eq!(usages.len(), 1, "one message, two entries");
        assert_eq!(
            (usages[0].cache_write_5m, usages[0].cache_write_1h),
            (10.0, 30.0)
        );
        assert!(evs.iter().any(|e| matches!(e, TxEvent::ApiError { kind: Some(k), message } if k == "rate_limit" && message == "API Error: 429")));
        assert!(evs.iter().any(|e| matches!(
            e,
            TxEvent::TurnEnd {
                duration_ms: Some(4200)
            }
        )));
    }

    #[test]
    fn background_tasks_start_and_end_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let call = |id: &str, cmd: &str| {
            json!({"type": "assistant", "uuid": format!("c-{id}"), "message": {"id": format!("m-{id}"), "role": "assistant",
                "content": [{"type": "tool_use", "id": id, "name": "Bash",
                    "input": {"command": cmd, "description": "wait", "run_in_background": true}}]}})
        };
        let note = |task: &str| {
            format!(
                "<task-notification>\n<task-id>{task}</task-id>\n<status>completed</status>\n</task-notification>"
            )
        };
        write_lines(
            &main,
            &[
                call("t1", "sleep 5"),
                tool_result_entry(
                    "t1",
                    "Command running in background with ID: bg1. Output is being written to: /x",
                    json!({}),
                ),
                call("t2", "sleep 6"),
                tool_result_entry(
                    "t2",
                    "Command running in background with ID: bg2. Output is being written to: /y",
                    json!({}),
                ),
                // Ended while idle: a plain user message.
                json!({"type": "user", "uuid": "n1", "message": {"role": "user", "content": note("bg1")}}),
                // Ended while busy: a queued_command attachment.
                json!({"type": "attachment", "uuid": "n2", "attachment": {"type": "queued_command",
                    "commandMode": "task-notification", "prompt": note("bg2")}}),
            ],
        );
        let mut t = Tailer::new(&main);
        t.poll();
        let tasks = t.tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            (tasks[0].kind, tasks[0].command.as_deref()),
            (TaskKind::Shell, Some("sleep 5"))
        );
        assert!(tasks.iter().all(|t| t.status == "completed"), "{tasks:?}");
    }

    /// A Monitor is a task of its own kind; its events (notifications without a status) do not
    /// end it, the one saying its stream ended does.
    #[test]
    fn a_monitor_runs_until_its_stream_ends() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let user = |uuid: &str, text: String| json!({"type": "user", "uuid": uuid, "message": {"role": "user", "content": text}});
        let note = |inner: &str| {
            format!("<task-notification>\n<task-id>bm1</task-id>\n{inner}\n</task-notification>")
        };
        let mut lines = vec![
            json!({"type": "assistant", "uuid": "c-t1", "message": {"id": "m-t1", "role": "assistant",
                "content": [{"type": "tool_use", "id": "t1", "name": "Monitor",
                    "input": {"command": "tail -F app.log", "description": "app errors", "timeout_ms": 900000}}]}}),
            tool_result_entry(
                "t1",
                "Monitor started (task bm1, timeout 900000ms). You will be notified on each event.",
                json!({"taskId": "bm1", "timeoutMs": 900000, "persistent": false}),
            ),
            user(
                "e1",
                note("<summary>Monitor event: \"app errors\"</summary>\n<event>ERROR boom</event>"),
            ),
        ];
        write_lines(&main, &lines);
        let mut t = Tailer::new(&main);
        t.poll();
        let task = &t.tasks()[0];
        assert_eq!(
            (
                task.kind,
                task.description.as_deref(),
                task.command.as_deref(),
                task.status.as_str()
            ),
            (
                TaskKind::Monitor,
                Some("app errors"),
                Some("tail -F app.log"),
                "running"
            )
        );
        // Appended: the stream ended.
        lines.push(user(
            "e2",
            note("<status>completed</status>\n<summary>Monitor \"app errors\" stream ended</summary>"),
        ));
        write_lines(&main, &lines);
        t.poll();
        assert_eq!(t.tasks()[0].status, "completed");
    }

    /// A foreground command moved to the background on its timeout is a task too, and TaskStop
    /// (which sends no notification for a shell) ends it.
    #[test]
    fn a_task_moved_to_the_background_and_one_stopped_with_task_stop() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let call = |id: &str, name: &str, input: Value| {
            json!({"type": "assistant", "uuid": format!("c-{id}"), "message": {"id": format!("m-{id}"), "role": "assistant",
                "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}})
        };
        write_lines(
            &main,
            &[
                call(
                    "t1",
                    "Bash",
                    json!({"command": "make e2e", "description": "E2E"}),
                ),
                tool_result_entry(
                    "t1",
                    "running…\nCommand did not complete within its 120s timeout and was moved to the background (ID: bg1). Output is being written to: /x",
                    json!({}),
                ),
                call(
                    "t2",
                    "Bash",
                    json!({"command": "sleep 60", "description": "wait", "run_in_background": true}),
                ),
                tool_result_entry(
                    "t2",
                    "Command running in background with ID: bg2. Output is being written to: /y",
                    json!({}),
                ),
                call("t3", "TaskStop", json!({"task_id": "bg2"})),
                tool_result_entry(
                    "t3",
                    "{\"message\":\"Successfully stopped task: bg2 (sleep 60)\"}",
                    json!({}),
                ),
            ],
        );
        let mut t = Tailer::new(&main);
        t.poll();
        let status: Vec<_> = t
            .tasks()
            .iter()
            .map(|t| (t.id.as_str(), t.status.as_str()))
            .collect();
        assert_eq!(status, [("bg1", "running"), ("bg2", "stopped")]);
    }

    /// `get_tasks` puts `BgTask` on the wire: the kind is a lowercase string, `command` only
    /// for a shell or a Monitor.
    #[test]
    fn background_tasks_serialize_as_on_the_wire() {
        let task = |kind, command: Option<&str>| BgTask {
            id: "t1".into(),
            kind,
            tool_use_id: "toolu_1".into(),
            description: Some("d".into()),
            command: command.map(str::to_string),
            status: "running".into(),
        };
        let all = [
            task(TaskKind::Shell, Some("sleep 5")),
            task(TaskKind::Agent, None),
            task(TaskKind::Monitor, Some("tail -F x")),
            task(TaskKind::Workflow, None),
        ];
        assert_eq!(
            serde_json::to_string(&all).unwrap(),
            concat!(
                r#"[{"id":"t1","kind":"shell","tool_use_id":"toolu_1","description":"d","command":"sleep 5","status":"running"},"#,
                r#"{"id":"t1","kind":"agent","tool_use_id":"toolu_1","description":"d","status":"running"},"#,
                r#"{"id":"t1","kind":"monitor","tool_use_id":"toolu_1","description":"d","command":"tail -F x","status":"running"},"#,
                r#"{"id":"t1","kind":"workflow","tool_use_id":"toolu_1","description":"d","status":"running"}]"#
            )
        );
    }

    #[test]
    fn a_compact_boundary_is_not_a_branch() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let (_, evs) = tail_then(
            &main,
            &history(),
            &[
                system("c1", None, "compact_boundary", Some("a2")),
                prompt("u3", Some("c1"), "after compact", 5),
            ],
        );
        assert!(
            !evs.iter().any(|e| matches!(e, TxEvent::Branch { .. })),
            "{evs:?}"
        );
    }

    #[test]
    fn a_child_written_before_its_parent_is_not_a_branch() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        // Read together: the teammate prompt comes first, then the entry it hangs off.
        let (_, evs) = tail_then(
            &main,
            &history(),
            &[
                prompt("t1", Some("h1"), "teammate says hi", 5),
                system("h1", Some("a2"), "stop_hook_summary", None),
            ],
        );
        assert!(
            !evs.iter().any(|e| matches!(e, TxEvent::Branch { .. })),
            "{evs:?}"
        );
        let texts: Vec<_> = main_blocks(&main, 0)
            .0
            .into_iter()
            .filter_map(|b| b.text)
            .collect();
        assert_eq!(texts.last().map(String::as_str), Some("teammate says hi"));
        // Read apart: the parent is not there yet when the child is read.
        let main2 = dir.path().join("s2.jsonl");
        let mut lines = history();
        lines.push(prompt("t1", Some("h1"), "teammate says hi", 5));
        let (mut t, evs) = tail_then(&main2, &history(), &lines[history().len()..]);
        assert!(!evs.iter().any(|e| matches!(e, TxEvent::Branch { .. })));
        assert!(
            main_blocks(&main2, 0).0.len() >= 4,
            "the conversation is still there"
        );
        lines.push(system("h1", Some("a2"), "stop_hook_summary", None));
        write_lines(&main2, &lines);
        assert!(!t.poll().iter().any(|e| matches!(e, TxEvent::Branch { .. })));
    }

    #[test]
    fn resending_the_same_prompt_is_not_a_branch_and_cursors_still_work() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let first = vec![
            prompt("u1", None, "first", 1),
            reply("a1", "u1", "one", 2),
            prompt("u2", Some("a1"), "again", 3),
            interruption("i2", "u2"),
        ];
        write_lines(&main, &first);
        let (_, cursor) = main_blocks(&main, 0);
        let (_, evs) = tail_then(&main, &first, &[prompt("u3", Some("a1"), "again", 4)]);
        assert!(
            !evs.iter().any(|e| matches!(e, TxEvent::Branch { .. })),
            "{evs:?}"
        );
        let after: Vec<_> = main_blocks(&main, cursor)
            .0
            .into_iter()
            .map(|b| b.uuid)
            .collect();
        assert_eq!(after, ["u3"]);
    }

    fn tool_result_entry(tool_use_id: &str, content: &str, extra: Value) -> Value {
        let mut v = json!({"type": "user", "uuid": format!("r-{tool_use_id}"),
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tool_use_id, "content": content}]}});
        if let (Value::Object(m), Value::Object(x)) = (&mut v, extra) {
            m.extend(x);
        }
        v
    }

    #[test]
    fn persisted_output_points_only_into_the_sessions_tool_results() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let results = dir.path().join("s").join("tool-results");
        std::fs::create_dir_all(&results).unwrap();
        let full = results.join("abc.txt");
        std::fs::write(&full, "FULL OUTPUT").unwrap();
        let preview = |p: &Path| {
            format!(
                "<persisted-output>\nOutput too large (42.4KB). Full output saved to: {}\n\nPreview (first 2KB):\nFULL",
                p.display()
            )
        };
        let outside = dir.path().join("elsewhere.txt");
        std::fs::write(&outside, "SECRET").unwrap();
        write_lines(
            &main,
            &[
                tool_result_entry("t1", &preview(&full), json!({})),
                tool_result_entry("t2", &preview(&outside), json!({})),
                tool_result_entry("t3", "short", json!({})),
            ],
        );
        let mut t = Tailer::new(&main);
        let evs = t.poll();
        let bs = blocks(&evs);
        let p = bs[0].persisted.as_ref().unwrap();
        assert_eq!((p.path.as_str(), p.size), (full.to_str().unwrap(), 11));
        assert!(
            bs[0]
                .content
                .as_deref()
                .unwrap()
                .starts_with("<persisted-output>")
        );
        assert!(
            bs[1].persisted.is_none(),
            "a path outside tool-results/ is not followed"
        );
        assert_eq!(t.tool_result("t1").unwrap().0, "FULL OUTPUT");
        assert!(
            t.tool_result("t2")
                .unwrap()
                .0
                .starts_with("<persisted-output>")
        );
        assert_eq!(t.tool_result("t3").unwrap(), ("short".to_string(), None));
        assert!(t.tool_result("nope").is_none());
    }

    #[test]
    fn workflow_runs_are_reported_with_their_subagent_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let sdir = dir.path().join("s");
        std::fs::create_dir_all(sdir.join("workflows")).unwrap();
        let agents = sdir.join("subagents").join("workflows").join("wf_1");
        std::fs::create_dir_all(&agents).unwrap();
        let wf = |status: &str, state: &str| {
            json!({"workflowName": "review", "status": status,
                "phases": [{"title": "Read", "detail": "parallel"}],
                "workflowProgress": [
                    {"type": "workflow_phase", "index": 1, "title": "Read"},
                    {"type": "workflow_agent", "agentId": "ag1", "label": "reader",
                        "phaseIndex": 1, "state": state}]})
            .to_string()
        };
        std::fs::write(sdir.join("workflows/wf_1.json"), wf("running", "running")).unwrap();
        write_lines(
            &main,
            &[tool_result_entry(
                "toolu_wf",
                "started",
                json!({"toolUseResult": {"runId": "wf_1"}}),
            )],
        );
        write_lines(
            &agents.join("agent-ag1.jsonl"),
            &[prompt("x1", None, "read it", 1)],
        );
        let mut t = Tailer::new(&main);
        let evs = t.poll();
        let w = evs
            .iter()
            .find_map(|e| match e {
                TxEvent::Workflow(v) => Some(v.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(w["run_id"], "wf_1");
        assert_eq!(w["tool_use_id"], "toolu_wf");
        assert_eq!(w["status"], "running");
        assert_eq!(w["phases"][0]["title"], "Read");
        assert_eq!(w["agents"][0]["label"], "reader");
        let sub = blocks(&evs)
            .into_iter()
            .find(|b| b.agent_id.as_deref() == Some("ag1"))
            .unwrap();
        assert_eq!(sub.run_id.as_deref(), Some("wf_1"));
        assert_eq!(sub.parent_tool_use_id.as_deref(), Some("toolu_wf"));
        // Unchanged files: nothing new; a new state: a new snapshot.
        t.last_discover = None;
        assert!(!t.poll().iter().any(|e| matches!(e, TxEvent::Workflow(_))));
        std::fs::write(sdir.join("workflows/wf_1.json"), wf("completed", "done")).unwrap();
        t.last_discover = None;
        let w = t
            .poll()
            .into_iter()
            .find_map(|e| match e {
                TxEvent::Workflow(v) => Some(v),
                _ => None,
            })
            .unwrap();
        assert_eq!(w["status"], "completed");
    }
}
