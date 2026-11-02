//! Incremental tail of a Session's transcript (main file, Subagent Run files, Workflow Run files)
//! producing Blocks and the few markers the state machine needs. Reads only complete lines.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use cv_core::assemble::{LoadedFile, WorkflowFiles, skeleton, tree, workflow};
use cv_core::model::FileRole;
use cv_core::raw::{RawBlock, RawContent, RawEntry, decode_str};
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
    /// `system` / `turn_duration` in the main file: the end of a turn.
    TurnEnd,
    /// `queue-operation` / `enqueue`: input typed while busy was accepted.
    Enqueued {
        content: String,
    },
    /// A user prompt entry in the main file (string content).
    UserPrompt {
        uuid: String,
        text: String,
    },
    /// The Main Line moved to another Branch: blocks emitted after `at_uuid` are off it.
    Branch {
        at_uuid: String,
    },
    /// A Workflow Run's state changed (the whole state, protocol shape).
    Workflow(Value),
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
    /// uuid of the last main-file entry read.
    last_uuid: Option<String>,
    /// Main-file entries that produced blocks, in order, with the prompt text for prompts.
    emitted: Vec<(String, Option<String>)>,
    emitted_set: HashSet<String>,
}

/// Directory holding the Subagent Run files of `main` (`<main without .jsonl>/subagents`).
pub fn subagents_dir(main: &Path) -> PathBuf {
    main.with_extension("").join("subagents")
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
            last_uuid: None,
            emitted: Vec::new(),
            emitted_set: HashSet::new(),
        }
    }

    /// `<sessionId>/` next to the main file: subagents, workflows, tool-results.
    fn session_dir(&self) -> PathBuf {
        self.main.with_extension("")
    }

    pub fn main_path(&self) -> &Path {
        &self.main
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
        // A file that shrank was replaced: start it over with fresh state.
        if std::fs::metadata(&self.main).map(|m| m.len()).unwrap_or(0) < self.offset {
            self.offset = 0;
            self.pending.clear();
            self.last_text = None;
            self.last_uuid = None;
            self.emitted.clear();
            self.emitted_set.clear();
        }
        if let Ok(chunk) = read_lines(&self.main, self.offset) {
            self.offset = chunk.consumed_to;
            self.capped |= chunk.capped;
            let mut fork: Option<(String, String)> = None;
            for l in chunk.lines {
                let before = out.len();
                if let Some(f) = self.fork_candidate(&l) {
                    fork = Some(f);
                }
                self.on_entry(&l, None, None, None, &mut out);
                self.note_emitted(&l, &out[before..]);
            }
            if let Some((at, text)) = fork
                && self.branch_moved(&text)
            {
                out.push(TxEvent::Branch { at_uuid: at });
            }
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
                && sf.checked_at.is_some_and(|t| now.duration_since(t) < COLD_CHECK)
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

    /// A main-file user prompt whose parent is not the entry before it may start a new Branch:
    /// `(parent uuid, prompt text)`.
    fn fork_candidate(&self, line: &Line) -> Option<(String, String)> {
        let e = &line.entry;
        if e.entry_type() != "user" || e.is_meta == Some(true) {
            return None;
        }
        let Some(RawContent::Text(text)) = e.message.as_ref().and_then(|m| m.content.as_ref())
        else {
            return None;
        };
        let parent = e.parent_uuid.as_ref()?;
        (self.last_uuid.is_some() && self.last_uuid.as_ref() != Some(parent))
            .then(|| (parent.clone(), text.clone()))
    }

    /// Records the main-file entry behind blocks just produced (for Branch checks).
    fn note_emitted(&mut self, line: &Line, new: &[TxEvent]) {
        let e = &line.entry;
        if let Some(u) = &e.uuid {
            self.last_uuid = Some(u.clone());
        }
        let Some(uuid) = e.uuid.clone() else {
            return;
        };
        let mut prompt = None;
        let mut any = false;
        for ev in new {
            match ev {
                TxEvent::Block(b) if b.agent_id.is_none() => any = true,
                TxEvent::UserPrompt { text, .. } => prompt = Some(text.clone()),
                _ => {}
            }
        }
        if any && self.emitted_set.insert(uuid.clone()) {
            self.emitted.push((uuid, prompt));
        }
    }

    /// Recomputes the Main Line (cv-core rules); true when blocks already emitted fell off it.
    /// A Branch that only resent the same prompt with no reply does not count.
    fn branch_moved(&mut self, new_prompt: &str) -> bool {
        let Some(abandoned) = off_main_uuids(&self.main) else {
            return false;
        };
        let off: Vec<(String, Option<String>)> = self
            .emitted
            .iter()
            .filter(|(u, _)| abandoned.contains(u))
            .cloned()
            .collect();
        if off.is_empty() {
            return false;
        }
        for (u, _) in &off {
            self.emitted_set.remove(u);
        }
        self.emitted.retain(|(u, _)| !abandoned.contains(u));
        let resend = off
            .iter()
            .all(|(_, p)| p.as_deref().is_some_and(|t| t.trim() == new_prompt.trim()));
        !resend
    }

    fn poll_workflows(&mut self, out: &mut Vec<TxEvent>) {
        let dir = self.session_dir();
        let json_dir = dir.join("workflows");
        let journal_dir = dir.join("subagents").join("workflows");
        let mut runs: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&json_dir) {
            for e in rd.flatten() {
                if let Some(r) = e.file_name().to_str().and_then(|n| n.strip_suffix(".json")) {
                    runs.push(r.to_string());
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(&journal_dir) {
            for e in rd.flatten() {
                if e.path().join("journal.jsonl").is_file()
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
            let journal_path = journal_dir.join(&run).join("journal.jsonl");
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
        let dir = subagents_dir(&self.main);
        self.discover_in(&dir, None);
        if let Ok(rd) = std::fs::read_dir(dir.join("workflows")) {
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
            let Some(stem) = name.strip_suffix(".jsonl") else {
                continue;
            };
            let Some(agent_id) = stem.strip_prefix("agent-") else {
                continue;
            };
            if let Some(s) = self.subs.get_mut(agent_id) {
                if s.parent_tool_use_id.is_none() && s.run_id.is_none() {
                    s.parent_tool_use_id = read_meta_tool_use_id(dir, stem);
                }
                continue;
            }
            let parent = if run.is_none() {
                read_meta_tool_use_id(dir, stem)
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
                out.push(TxEvent::TurnEnd);
            }
            "queue-operation" if agent.is_none() => {
                if line.operation.as_deref() == Some("enqueue") {
                    let content = decode_str(e.content.as_deref()).unwrap_or_default();
                    out.push(TxEvent::Enqueued { content });
                }
            }
            "attachment" if agent.is_none() => {
                let Some(att) = e.attachment.as_deref() else {
                    return;
                };
                let Ok(v) = serde_json::from_str::<Value>(att.get()) else {
                    return;
                };
                if v.get("type").and_then(Value::as_str) != Some("queued_command") {
                    return;
                }
                if v.get("commandMode")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m != "prompt")
                {
                    return;
                }
                let text = match v.get("prompt") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => return,
                };
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
                match &msg.content {
                    Some(RawContent::Text(s)) if !assistant => {
                        if is_command_record(s) {
                            return;
                        }
                        let mut b = Block::new(BlockKind::User, &uuid, 0);
                        b.text = Some(s.clone());
                        // Claude Code's own interruption notes are not prompts and open no turn.
                        if agent.is_none() && !s.starts_with("[Request interrupted by user") {
                            // A new prompt starts a new turn: its last text is not the previous one's.
                            self.last_text = None;
                            out.push(TxEvent::UserPrompt {
                                uuid: uuid.clone(),
                                text: s.clone(),
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
                        for (i, rb) in blocks.iter().enumerate() {
                            if let Some(b) = self.block_of(rb, &uuid, i, assistant, agent) {
                                out.push(TxEvent::Block(Box::new(tag(b))));
                            }
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
                self.pending.remove(&tid);
                let mut b = Block::new(BlockKind::ToolResult, uuid, i);
                let text = result_text(rb.content.as_deref());
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

/// Blocks of the main file on the Main Line, from the beginning (for `get_messages`).
pub fn main_blocks(main: &Path) -> Vec<Block> {
    let mut t = Tailer::new(main);
    let mut v = VecDeque::new();
    if let Ok(chunk) = read_lines(main, 0) {
        t.offset = chunk.consumed_to;
        let mut out = Vec::new();
        for l in chunk.lines {
            t.on_entry(&l, None, None, None, &mut out);
        }
        let off = off_main_uuids(main).unwrap_or_default();
        for ev in out {
            if let TxEvent::Block(b) = ev
                && !off.contains(&b.uuid)
            {
                v.push_back(*b);
            }
        }
    }
    v.into()
}

/// uuids of the entries of `main` on an abandoned Branch: in the Main Line's tree (cv-core's
/// rules) but not on the Main Line. Entries cut off by a dangling parent are another tree, not a
/// Branch, and are not included. `None` if unreadable.
pub fn off_main_uuids(main: &Path) -> Option<HashSet<String>> {
    let chunk = cv_core::parse::read_entries(main, 0).ok()?;
    let files = [LoadedFile {
        path: main.to_path_buf(),
        role: FileRole::Main,
        agent_id: None,
        records: chunk.records,
    }];
    let s = skeleton::build(&files);
    let on_main = tree::main_set(&s);
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
    let main_root = root_of(*s.main_path.first()?);
    Some(
        (0..s.nodes.len())
            .filter(|&i| root_of(i) == main_root && !on_main.contains(&s.nodes[i].uuid))
            .map(|i| s.nodes[i].uuid.clone())
            .collect(),
    )
}

/// The file a `<persisted-output>` preview points to, only when it is in this Session's
/// `tool-results/` (a tool's own output could imitate the preview).
fn persisted_path(text: &str, session_dir: &Path) -> Option<PathBuf> {
    if !text.trim_start().starts_with("<persisted-output>") {
        return None;
    }
    const MARK: &str = "Full output saved to: ";
    let rest = &text[text.find(MARK)? + MARK.len()..];
    let p = PathBuf::from(rest.lines().next()?.trim());
    let dir = session_dir.join("tool-results");
    let ok = p.parent() == Some(dir.as_path())
        && p.file_name().is_some()
        && !p.components().any(|c| matches!(c, std::path::Component::ParentDir));
    ok.then_some(p)
}

/// A text file read with the block cap (read-only).
fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    f.take(MAX_BLOCK_TEXT as u64 + 4).read_to_end(&mut buf).ok()?;
    Some(cap(String::from_utf8_lossy(&buf).into_owned()))
}

fn read_meta_tool_use_id(dir: &Path, stem: &str) -> Option<String> {
    let bytes = std::fs::read(dir.join(format!("{stem}.meta.json"))).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("toolUseId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One complete transcript line: cv-core's tolerant entry plus the `operation` field of
/// `queue-operation` entries (which cv-core does not model).
struct Line {
    entry: RawEntry,
    operation: Option<String>,
}

#[derive(serde::Deserialize)]
struct OperationField {
    #[serde(default)]
    operation: Option<String>,
}

struct Chunk {
    lines: Vec<Line>,
    consumed_to: u64,
    /// The read stopped at the size cap: more complete lines may follow.
    capped: bool,
}

/// Reads the complete lines of `path` from byte `from` (read-only; a partial last line is left
/// for the next read; lines that are not JSON objects are skipped).
fn read_lines(path: &Path, from: u64) -> std::io::Result<Chunk> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len <= from {
        return Ok(Chunk {
            lines: Vec::new(),
            consumed_to: from.min(len),
            capped: false,
        });
    }
    f.seek(SeekFrom::Start(from))?;
    let want = (len - from).min(MAX_READ);
    let mut buf = Vec::with_capacity(want as usize);
    f.take(want).read_to_end(&mut buf)?;
    let end = match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        // A single line longer than the read cap: skip what was read (the rest of the line then
        // fails to parse and is skipped too) instead of re-reading it forever.
        None if want == MAX_READ => {
            return Ok(Chunk {
                lines: Vec::new(),
                consumed_to: from + want,
                capped: true,
            });
        }
        None => {
            return Ok(Chunk {
                lines: Vec::new(),
                consumed_to: from,
                capped: false,
            });
        }
    };
    let mut lines = Vec::new();
    for raw in buf[..end].split(|&b| b == b'\n') {
        let raw = raw.trim_ascii();
        if raw.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<RawEntry>(raw) else {
            continue;
        };
        let operation = if entry.entry_type() == "queue-operation" {
            serde_json::from_slice::<OperationField>(raw)
                .ok()
                .and_then(|o| o.operation)
        } else {
            None
        };
        lines.push(Line { entry, operation });
    }
    Ok(Chunk {
        lines,
        consumed_to: from + end as u64,
        capped: want == MAX_READ,
    })
}

/// Slash-command bookkeeping that Claude Code records as user entries.
fn is_command_record(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with("<local-command-caveat>")
        || t.starts_with("<command-name>")
        || t.starts_with("<command-message>")
        || t.starts_with("<local-command-stdout>")
        || t.starts_with("<local-command-stderr>")
        || t.starts_with("<bash-input>")
        || t.starts_with("<bash-stdout>")
        || t.starts_with("<bash-stderr>")
}

fn result_text(raw: Option<&serde_json::value::RawValue>) -> String {
    let Some(raw) = raw else {
        return String::new();
    };
    match serde_json::from_str::<Value>(raw.get()) {
        Ok(Value::String(s)) => s,
        Ok(Value::Array(a)) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
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
        let turn_ends = evs.iter().filter(|e| matches!(e, TxEvent::TurnEnd)).count();
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
    fn main_blocks_reads_whole_file() {
        let bs = main_blocks(&fixture("steer_ask_permission.jsonl"));
        assert!(bs.len() > 20);
        assert_eq!(bs[0].kind, BlockKind::User);
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

    #[test]
    fn rewind_reports_a_branch_and_get_messages_keeps_the_main_line() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let mut lines = vec![
            prompt("u1", None, "first", 1),
            reply("a1", "u1", "one", 2),
            prompt("u2", Some("a1"), "second", 3),
            reply("a2", "u2", "two", 4),
        ];
        write_lines(&main, &lines);
        let mut t = Tailer::new(&main);
        assert!(!t.poll().iter().any(|e| matches!(e, TxEvent::Branch { .. })));
        // Rewind to after "one" and ask something else.
        lines.push(prompt("u3", Some("a1"), "other", 5));
        lines.push(reply("a3", "u3", "three", 6));
        write_lines(&main, &lines);
        let evs = t.poll();
        assert!(
            evs.iter()
                .any(|e| matches!(e, TxEvent::Branch { at_uuid } if at_uuid == "a1")),
            "{evs:?}"
        );
        let texts: Vec<_> = main_blocks(&main)
            .into_iter()
            .filter_map(|b| b.text)
            .collect();
        assert_eq!(texts, ["first", "one", "other", "three"]);
    }

    #[test]
    fn resending_the_same_prompt_is_not_a_branch() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let mut lines = vec![
            prompt("u1", None, "first", 1),
            reply("a1", "u1", "one", 2),
            prompt("u2", Some("a1"), "again", 3),
        ];
        write_lines(&main, &lines);
        let mut t = Tailer::new(&main);
        t.poll();
        lines.push(prompt("u3", Some("a1"), "again", 4));
        write_lines(&main, &lines);
        assert!(!t.poll().iter().any(|e| matches!(e, TxEvent::Branch { .. })));
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
            format!("<persisted-output>\nOutput too large (42.4KB). Full output saved to: {}\n\nPreview (first 2KB):\nFULL", p.display())
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
        assert!(bs[0].content.as_deref().unwrap().starts_with("<persisted-output>"));
        assert!(bs[1].persisted.is_none(), "a path outside tool-results/ is not followed");
        assert_eq!(t.tool_result("t1").unwrap().0, "FULL OUTPUT");
        assert!(t.tool_result("t2").unwrap().0.starts_with("<persisted-output>"));
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
        write_lines(&agents.join("agent-ag1.jsonl"), &[prompt("x1", None, "read it", 1)]);
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
