//! Incremental tail of a Session's transcript (main file, Subagent Run files, Workflow Run files)
//! producing Blocks and the few markers the state machine needs. Reads only complete lines.
//!
//! [`Tailer`] follows the files and hands each entry to what keeps its own state: `block` (content
//! units, pending tools), `queue` (Claude Code's input queue), `tasks` (background tasks),
//! `teammates`, `branch` (the Main Line moving), `workflows`; `read` reads the files and `records`
//! reads Claude Code's own records.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ::transcript::assemble::{nodes, prompt};
use ::transcript::layout::{self, AgentFile};
use ::transcript::raw::{RawBlock, RawContent, RawEntry, decode_str};
use serde_json::Value;

mod block;
mod branch;
mod queue;
mod read;
mod records;
mod tasks;
mod teammates;
#[cfg(test)]
mod tests;
mod workflows;

pub use self::block::{Block, BlockKind, MAX_BLOCK_TEXT, PendingTool, Persisted};
pub use self::branch::off_main_uuids;
pub use self::queue::{QueueItem, QueueKind, Queued, queue_item};
pub use self::read::entry_uuids;
pub use self::records::local_setting;
pub use self::tasks::{BgTask, TaskKind, monitor_notice, task_notification};

use self::block::{cap, image_type, persisted_path};
use self::branch::Branches;
use self::queue::Queue;
use self::read::{Line, read_capped, read_lines, read_meta_tool_use_id};
use self::records::{bash_line, command_line};
use self::tasks::Tasks;
use self::teammates::Teammates;
use self::workflows::Workflows;

/// A Subagent file unchanged this long is checked only every [`COLD_CHECK`].
const COLD_AFTER: Duration = Duration::from_secs(60);
const COLD_CHECK: Duration = Duration::from_secs(5);
/// Directory listings (new Subagent / Workflow files) at most this often.
const DISCOVER_EVERY: Duration = Duration::from_millis(500);

/// What a poll found, in transcript order.
#[derive(Debug, Clone, PartialEq)]
pub enum TxEvent {
    Block(Box<Block>),
    /// `system` / `turn_duration` in the main file: the end of a turn, with its duration.
    TurnEnd {
        duration_ms: Option<u64>,
    },
    /// A `!` command's output in the main file (`<bash-stdout>` / `<bash-stderr>`), under its
    /// `<bash-input>` record (`parent_uuid`).
    BashOutput {
        parent_uuid: Option<String>,
        stdout: String,
        stderr: String,
    },
    /// `system` / `compact_boundary` in the main file: the conversation was compacted (`trigger`:
    /// `manual` or `auto`), with the context size before and after.
    Compacted {
        trigger: Option<String>,
        pre_tokens: Option<u64>,
        post_tokens: Option<u64>,
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
        /// The number ccrpc gave it (see [`Queued`]).
        id: u64,
        content: String,
    },
    /// Claude Code's input queue changed (main line): what waits in it now, oldest first, task
    /// notifications included.
    Queue(Vec<Queued>),
    /// `queue-operation` / `popAll`: `↑` in the TUI took this item back into the input box (one
    /// record per item taken; what cannot be edited there stays queued).
    Popped {
        id: u64,
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

/// Where an entry was read from: the main file, or a Subagent Run's file.
#[derive(Clone, Copy)]
struct Origin<'a> {
    /// The Subagent Run; `None` for the main file.
    agent: Option<&'a str>,
    /// The tool call that started the run.
    parent: Option<&'a str>,
    /// The Workflow Run the Subagent Run belongs to.
    run: Option<&'a str>,
}

impl Origin<'_> {
    const MAIN: Origin<'static> = Origin {
        agent: None,
        parent: None,
        run: None,
    };

    fn main(&self) -> bool {
        self.agent.is_none()
    }

    /// `b` marked with where it comes from.
    fn tag(&self, mut b: Block) -> Block {
        b.agent_id = self.agent.map(str::to_string);
        b.parent_tool_use_id = self.parent.map(str::to_string);
        b.run_id = self.run.map(str::to_string);
        b
    }
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
    workflows: Workflows,
    /// tool_use id → file holding the full output.
    persisted: HashMap<String, PathBuf>,
    branches: Branches,
    /// Read the main file only (no Subagent / Workflow files).
    main_only: bool,
    /// Assistant message ids whose usage was reported.
    usage_seen: HashSet<String>,
    tasks: Tasks,
    teammates: Teammates,
    /// Byte offset of every main-file entry, kept when `main_only` or `keep_offsets`.
    offsets: HashMap<String, u64>,
    keep_offsets: bool,
    queue: Queue,
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
            workflows: Workflows::default(),
            persisted: HashMap::new(),
            branches: Branches::default(),
            main_only: false,
            offsets: HashMap::new(),
            keep_offsets: false,
            usage_seen: HashSet::new(),
            tasks: Tasks::default(),
            teammates: Teammates::default(),
            queue: Queue::default(),
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
    pub fn queue(&self) -> &[Queued] {
        self.queue.items()
    }

    /// Background tasks seen so far.
    pub fn tasks(&self) -> &[BgTask] {
        self.tasks.list()
    }

    /// Tasks a Monitor's notification named: how one a plugin started (no tool call of its own,
    /// listed as `shell` in Stop's `background_tasks`) is told from a shell.
    pub fn monitors(&self) -> &HashSet<String> {
        self.tasks.monitors()
    }

    /// Whether the teammates spawned with `description` that have not exited are all idle.
    pub fn teammate_idle(&self, description: &str) -> bool {
        self.teammates.idle(description)
    }

    /// A task ccrpc stopped: `stopped` until Claude Code's own notice says otherwise.
    pub fn mark_stopped(&mut self, id: &str) {
        self.tasks.mark_stopped(id);
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
        self.workflows.forget();
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

    /// Claude Code moved the Session's files, its directory with the transcript: what has been
    /// read of them stays read.
    pub fn moved_to(&mut self, main: &Path) {
        let (old, new) = (self.main.with_extension(""), main.with_extension(""));
        let rebase = |p: &mut PathBuf| {
            if let Ok(rest) = p.strip_prefix(&old) {
                *p = new.join(rest);
            }
        };
        self.subs.values_mut().for_each(|s| rebase(&mut s.path));
        self.persisted.values_mut().for_each(rebase);
        self.main = main.to_path_buf();
    }

    pub fn poll(&mut self) -> Vec<TxEvent> {
        let mut out = Vec::new();
        self.capped = false;
        // A file that shrank was replaced: what was read from it starts over. What other files
        // gave (Subagent / Workflow progress, the Workflow Runs reported) and the usage already
        // reported carry over, so none of it is reported twice.
        // A missing file is not one: it may have moved ([`Tailer::moved_to`]).
        if std::fs::metadata(&self.main).is_ok_and(|m| m.len() < self.offset) {
            let old = std::mem::replace(self, Tailer::new(&self.main));
            self.copied_from(old.copied);
            self.queue.continue_numbering(&old.queue);
            self.main_only = old.main_only;
            self.keep_offsets = old.keep_offsets;
            self.subs = old.subs;
            self.workflows.keep_seen(old.workflows);
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
                    && self.branches.is_fork(l)
                    && let Some(at) = self.branches.moved(&self.main, l)
                {
                    out.push(TxEvent::Branch { at_uuid: at });
                }
                let before = out.len();
                self.on_entry(l, Origin::MAIN, &mut out);
                self.branches.note_emitted(l, &out[before..]);
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
                Some(r) => self.workflows.tool_of(r),
                None => sf.parent_tool_use_id.clone(),
            };
            if let Ok(chunk) = read_lines(&path, offset) {
                if let Some(s) = self.subs.get_mut(&id) {
                    s.offset = chunk.consumed_to;
                }
                self.capped |= chunk.capped;
                for l in chunk.lines {
                    let from = Origin {
                        agent: Some(&id),
                        parent: parent.as_deref(),
                        run: run.as_deref(),
                    };
                    self.on_entry(&l, from, &mut out);
                }
            }
        }
        if discover {
            let dir = self.session_dir();
            self.workflows.poll(&dir, &mut out);
        }
        out
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

    /// One entry of the main file or of a Subagent Run's.
    fn on_entry(&mut self, line: &Line, from: Origin<'_>, out: &mut Vec<TxEvent>) {
        let e = &line.entry;
        if from.main() {
            self.teammates.note_entry(e);
        }
        // A Subagent Run's code (Code Mode) is handed its events through the main queue too.
        if e.entry_type() == "user" && self.queue.settle_delivered(e) {
            out.push(TxEvent::Queue(self.queue.items().to_vec()));
        }
        match e.entry_type() {
            "system" if from.main() => self.on_system(e, out),
            "queue-operation" if from.main() => self.queue.on_operation(e, out),
            "attachment" if from.main() => self.on_queued(e, out),
            "user" | "assistant" => self.on_message(e, from, out),
            _ => {}
        }
    }

    /// A `system` entry of the main file: the end of a turn, a compaction, a local command.
    fn on_system(&mut self, e: &RawEntry, out: &mut Vec<TxEvent>) {
        let uuid = e.uuid.clone().unwrap_or_default();
        match e.subtype.as_deref() {
            Some("turn_duration") => {
                self.pending.retain(|_, p| p.agent_id.is_some());
                out.push(TxEvent::TurnEnd {
                    duration_ms: e.duration_ms.map(|d| d.max(0.0) as u64),
                });
            }
            Some("compact_boundary") => {
                let m = e.compact_metadata.clone().unwrap_or_default();
                let tokens = |t: Option<f64>| t.map(|t| t.max(0.0) as u64);
                out.push(TxEvent::Compacted {
                    trigger: m.trigger,
                    pre_tokens: tokens(m.pre_tokens),
                    post_tokens: tokens(m.post_tokens),
                });
            }
            // Newer Claude Code records a local slash command (`/rename`, `/model x`) this way.
            Some("local_command") => {
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

            _ => {}
        }
    }

    /// An `attachment` of the main file: what was queued while busy and delivered mid-turn.
    fn on_queued(&mut self, e: &RawEntry, out: &mut Vec<TxEvent>) {
        let uuid = e.uuid.clone().unwrap_or_default();

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
            self.tasks.note_notification(&text);
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

    /// A `user` or `assistant` entry.
    fn on_message(&mut self, e: &RawEntry, from: Origin<'_>, out: &mut Vec<TxEvent>) {
        let agent = from.agent;
        let uuid = e.uuid.clone().unwrap_or_default();
        let tag = |b: Block| from.tag(b);

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
            Some(RawContent::Text(s)) if !assistant => self.on_user_text(e, s, from, out),
            Some(RawContent::Text(s)) => {
                let mut b = Block::new(BlockKind::Text, &uuid, 0);
                b.text = Some(s.clone());
                if agent.is_none() {
                    self.last_text = Some(s.clone());
                }
                out.push(TxEvent::Block(Box::new(tag(b))));
            }
            Some(RawContent::Blocks(blocks)) => self.on_blocks(e, blocks, assistant, from, out),
            None => {}
        }
    }

    /// A user entry whose content is one string: a prompt, or one of Claude Code's own records.
    fn on_user_text(&mut self, e: &RawEntry, s: &str, from: Origin<'_>, out: &mut Vec<TxEvent>) {
        let agent = from.agent;
        let uuid = e.uuid.clone().unwrap_or_default();
        let tag = |b: Block| from.tag(b);

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
            } else if agent.is_none() {
                let t = s.trim_start();
                if t.starts_with("<bash-stdout>") || t.starts_with("<bash-stderr>") {
                    let part = |name| cap(prompt::tag(t, name).unwrap_or_default());
                    out.push(TxEvent::BashOutput {
                        parent_uuid: e.parent_uuid.clone(),
                        stdout: part("bash-stdout"),
                        stderr: part("bash-stderr"),
                    });
                }
            }
            return;
        }
        let s = &prompt::unwrap_pasted(s);
        if agent.is_none() {
            self.tasks.note_notification(s);
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

    /// An entry whose content is a list of blocks.
    fn on_blocks(
        &mut self,
        e: &RawEntry,
        blocks: &[RawBlock],
        assistant: bool,
        from: Origin<'_>,
        out: &mut Vec<TxEvent>,
    ) {
        let agent = from.agent;
        let uuid = e.uuid.clone().unwrap_or_default();
        let tag = |b: Block| from.tag(b);

        // A Workflow tool result names its run.
        if agent.is_none()
            && let Some(run_id) = e.tool_use_result_fields().and_then(|r| r.run_id)
            && let Some(tid) = blocks
                .iter()
                .find(|b| b.block_type() == "tool_result")
                .and_then(|b| b.tool_use_id.clone())
        {
            self.workflows.note_run(run_id, tid);
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
                if agent.is_none() {
                    self.tasks.on_tool_result(
                        tool.as_ref(),
                        &tid,
                        &text,
                        rb.is_error == Some(true),
                    );
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
