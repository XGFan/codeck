//! The Controlled Session: one engine thread owns the Holder connection, the Process File view
//! and the transcript tail; it executes commands in order and emits events. Commands that wait
//! keep ticking, so events keep flowing while they wait (follow-ups are only sent from the top
//! level loop, never from inside a waiting command).

use std::collections::{HashSet, VecDeque};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::analyze::{self, Analyzer};
use crate::hook::{self, HookMsg, HookServer};
use crate::layout::{self, Layout, LayoutKind, Library, Meaning};
use crate::locate::{self, Roots};
use crate::procfile::{self, ProcessFile};
use crate::pty::{self, HolderConn, Owner};
use crate::recognize::{self, PermissionChoice, PermissionDialog};
use crate::rpc::{self, Answer, Command, Decision, Out, Request, RpcError};
use crate::screen::Screen;
use crate::transcript::{self, BgTask, BlockKind, Tailer, TxEvent};
use crate::wake::{FsWatch, Wake};

pub const PROTOCOL: u32 = 2;
/// Longest wait between two looks while a command waits.
const TICK: Duration = Duration::from_millis(50);
/// Longest wait between two looks when idle (file, Holder and stdin events wake earlier).
const IDLE_TICK: Duration = Duration::from_millis(250);
/// A turn whose Session went idle without a Stop (Escape, a deny that interrupts) ends as aborted
/// after this long (Stop arrives before idle).
const ABORT_GRACE: Duration = Duration::from_millis(1500);
/// After Stop, how long to wait for the transcript to catch up (the turn's last message and its
/// `turn_duration` marker are written around the same moment) before `turn_end`.
const STOP_CATCH_UP: Duration = Duration::from_secs(2);
/// `input needed` without an AskUserQuestion from the plugin becomes `dialog` after this long.
const CLASSIFY_GRACE: Duration = Duration::from_millis(500);
/// A permission prompt whose dialog matches no request is reported as `dialog` (or analyzed)
/// after this long.
const MATCH_GRACE: Duration = Duration::from_secs(1);
/// After an answer went back through a hook, the Process File and the screen still show the
/// closing dialog for a moment: nothing is announced or reclassified meanwhile.
const ANSWER_SETTLE: Duration = Duration::from_millis(1500);
/// claude's Process File is there but its plugin never checked in: not controllable.
const CHECKIN_GRACE: Duration = Duration::from_secs(5);
/// A dialog ignores keys for a moment after it appears (against accidental answers): keys that
/// answer one wait until it has been up this long.
const DIALOG_SETTLE: Duration = Duration::from_millis(500);
/// No Process File yet while claude runs: report the startup screen after this long.
const STARTUP_DIALOG_AFTER: Duration = Duration::from_secs(3);
/// Keys are written apart so Claude Code reads each one on its own: at least this long, and up to
/// [`KEY_SETTLE`] while the screen has not reacted yet.
const KEY_MIN_GAP: Duration = Duration::from_millis(50);
const KEY_SETTLE: Duration = Duration::from_millis(80);
/// PTY size ccrpc sets when it takes the size (ADR-0001).
pub const DEFAULT_SIZE: (u16, u16) = (160, 48);
/// Claude Code versions the built-in layouts were probed on.
pub const TESTED_CLAUDE_VERSIONS: &[&str] = &["2.1.316"];
/// At most this many analyses an hour.
const MAX_ANALYSES_PER_HOUR: usize = 10;

/// Rows of the dialog at the bottom of the screen (from the last full-width rule), for telling
/// one unrecognized dialog from another.
fn dialog_signature(lines: &[String]) -> String {
    let top = lines
        .iter()
        .rposition(|l| {
            let t = l.trim();
            t.chars().count() > 20 && t.chars().all(|c| matches!(c, '─' | '▔' | '━'))
        })
        .map_or(0, |i| i + 1);
    lines[top..]
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
/// Tools that never ask for permission themselves (their inner tools do).
const NO_PERMISSION_TOOLS: &[&str] = &["AskUserQuestion", "Agent", "Task"];
/// Tools whose approval is a menu of ways to go on, answered as a choice.
const CHOICE_TOOLS: &[&str] = &["ExitPlanMode"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitKind {
    Permission,
    Question,
    Dialog,
}

impl WaitKind {
    fn name(&self) -> &'static str {
        match self {
            WaitKind::Permission => "permission",
            WaitKind::Question => "question",
            WaitKind::Dialog => "dialog",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Starting,
    Idle,
    Busy,
    Waiting { kind: WaitKind, waiting_for: String },
    Exited,
}

impl State {
    fn name(&self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Idle => "idle",
            State::Busy => "busy",
            State::Waiting { .. } => "waiting",
            State::Exited => "exited",
        }
    }

    fn to_json(&self) -> Value {
        match self {
            State::Waiting { kind, waiting_for } => {
                json!({"state": "waiting", "waiting": {"kind": kind.name(), "waiting_for": waiting_for}})
            }
            s => json!({"state": s.name()}),
        }
    }
}

/// What one turn did, reported with its `turn_end`.
#[derive(Default)]
struct TurnStats {
    started: Option<Instant>,
    duration_ms: Option<u64>,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_creation: f64,
    /// Estimated API cost; `None` once a message's model has no price.
    cost: Option<f64>,
    error: Option<Value>,
    aborted: bool,
    /// Claude Code's id of the prompt (`None`: a turn already running when ccrpc connected).
    prompt_id: Option<String>,
    /// `prompt`, or `task_notification` for a background task's result.
    trigger: &'static str,
    /// The transcript wrote the turn's `turn_duration` marker.
    marker_seen: bool,
    /// Opened for a steer expected to run after the previous Stop: dropped without `turn_end` if
    /// nothing comes of it.
    provisional: bool,
}

impl TurnStats {
    fn start(prompt_id: Option<String>, trigger: &'static str) -> TurnStats {
        TurnStats {
            started: Some(Instant::now()),
            cost: Some(0.0),
            prompt_id,
            trigger,
            ..Default::default()
        }
    }

    fn to_json(&self) -> Value {
        let measured = self.started.map(|t| t.elapsed().as_millis() as u64);
        json!({
            "prompt_id": self.prompt_id,
            "trigger": self.trigger,
            "error": self.error,
            "aborted": self.aborted,
            "duration_ms": self.duration_ms.or(measured),
            "usage": {
                "input_tokens": self.input as u64,
                "output_tokens": self.output as u64,
                "cache_read_input_tokens": self.cache_read as u64,
                "cache_creation_input_tokens": self.cache_creation as u64,
            },
            "api_cost": self.cost.map(|c| (c * 1e6).round() / 1e6),
        })
    }
}

/// Failure of a command: `(code, message)`.
type CmdErr = (&'static str, String);
type CmdResult = std::result::Result<Value, CmdErr>;

fn err<T>(code: &'static str, msg: impl Into<String>) -> std::result::Result<T, CmdErr> {
    Err((code, msg.into()))
}

/// Transcript markers used to confirm commands, each with a monotonic sequence number (compare by
/// sequence, never by list length: the lists are bounded).
#[derive(Default)]
struct Marks {
    seq: u64,
    prompts: VecDeque<(u64, String, String)>,
    enqueued: VecDeque<(u64, String)>,
}

impl Marks {
    fn prompt(&mut self, uuid: String, text: String) {
        self.seq += 1;
        self.prompts.push_back((self.seq, uuid, text));
        if self.prompts.len() > 64 {
            self.prompts.pop_front();
        }
    }

    fn enqueue(&mut self, text: String) {
        self.seq += 1;
        self.enqueued.push_back((self.seq, text));
        if self.enqueued.len() > 64 {
            self.enqueued.pop_front();
        }
    }

    fn prompt_after(&self, after: u64, text: &str) -> Option<&str> {
        self.prompts
            .iter()
            .rev()
            .find(|(s, _, t)| *s > after && t.trim() == text.trim())
            .map(|(_, u, _)| u.as_str())
    }

    /// A prompt sent with images: its recorded text may carry the `[Image #n]` placeholders.
    fn prompt_with_images_after(&self, after: u64, text: &str) -> Option<&str> {
        self.prompts
            .iter()
            .rev()
            .find(|(s, _, t)| *s > after && t.trim().ends_with(text.trim()))
            .map(|(_, u, _)| u.as_str())
    }

    fn enqueued_after(&self, after: u64, text: &str) -> bool {
        self.enqueued
            .iter()
            .any(|(s, t)| *s > after && t.trim() == text.trim())
    }
}

/// Text to look for in a permission dialog's body for this tool (whitespace removed, first 40
/// chars), or `None` when the tool's input gives nothing recognizable.
fn dialog_needle(name: &str, input: &Value) -> Option<String> {
    let field = match name {
        "Bash" => "command",
        "Edit" | "Write" | "MultiEdit" | "Read" | "NotebookEdit" => "file_path",
        "WebFetch" => "url",
        "Glob" | "Grep" => "pattern",
        _ => return None,
    };
    let v = input.get(field)?.as_str()?;
    let v = if field == "file_path" {
        v.rsplit('/').next().unwrap_or(v)
    } else {
        v
    };
    let n: String = v.chars().filter(|c| !c.is_whitespace()).take(40).collect();
    (!n.is_empty()).then_some(n)
}

/// AskUserQuestion's `answers` (question text → answer) for the caller's `answers`, one per
/// question: option labels (several joined by `, ` for a multi-select), or free text.
fn question_answers(
    questions: &[Value],
    answers: &[Answer],
) -> std::result::Result<serde_json::Map<String, Value>, String> {
    if questions.len() != answers.len() {
        return Err(format!("有 {} 道题，给了 {} 个答案", questions.len(), answers.len()));
    }
    let mut out = serde_json::Map::new();
    for (i, (q, a)) in questions.iter().zip(answers).enumerate() {
        let title = q.get("question").and_then(Value::as_str).unwrap_or_default();
        let multi = q.get("multiSelect").and_then(Value::as_bool).unwrap_or(false);
        let labels: Vec<&str> = q
            .get("options")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|o| o.get("label").and_then(Value::as_str)).collect())
            .unwrap_or_default();
        let text = match a {
            Answer::Options(chosen) => {
                if let Some(bad) = chosen.iter().find(|c| !labels.contains(&c.as_str())) {
                    return Err(format!("第 {} 题没有选项 {bad}", i + 1));
                }
                if !multi && chosen.len() != 1 {
                    return Err(format!("第 {} 题是单选", i + 1));
                }
                if chosen.is_empty() {
                    return Err(format!("第 {} 题至少要选一项", i + 1));
                }
                chosen.join(", ")
            }
            Answer::Other(t) => {
                let t = pty::sanitize(t).replace(['\n', '\t'], " ");
                if t.trim().is_empty() {
                    return Err(format!("第 {} 题的 other 为空", i + 1));
                }
                t
            }
        };
        out.insert(title.to_string(), json!(text));
    }
    Ok(out)
}

/// A tool call the plugin reported at PreToolUse (PermissionRequest has no tool_use id).
#[derive(Debug, Clone)]
struct PreTool {
    id: String,
    name: String,
    input: Value,
    agent_id: Option<String>,
}

/// A permission request (or AskUserQuestion) the plugin reported and that is not over yet.
#[derive(Debug)]
struct HookRequest {
    /// The tool_use id (paired from PreToolUse).
    id: String,
    tool_name: String,
    input: Value,
    agent_id: Option<String>,
    /// When the plugin reported it.
    at: Instant,
    /// The waiting hook: the answer goes back through it. `None`: answered on the screen (a
    /// background Subagent's request, ExitPlanMode, or the hook is gone).
    reply: Option<UnixStream>,
}

impl HookRequest {
    /// An approval of a tool (AskUserQuestion is a question; ExitPlanMode is a choice).
    fn is_approval(&self) -> bool {
        self.tool_name != "AskUserQuestion" && !CHOICE_TOOLS.contains(&self.tool_name.as_str())
    }
}

/// A prompt the plugin saw submitted: `(sequence, prompt_id, text)`.
type HookPrompt = (u64, String, String);

pub struct Engine {
    roots: Roots,
    /// ccrpc's state directory (check-ins, hook sockets).
    state_dir: PathBuf,
    /// The Session's hook socket, bound once claude is found.
    hooks: Option<HookServer>,
    pre_tools: VecDeque<PreTool>,
    requests: Vec<HookRequest>,
    hook_prompts: VecDeque<HookPrompt>,
    hook_seq: u64,
    /// Background tasks the plugin reported (started, ended) since ccrpc connected.
    hook_tasks: Vec<BgTask>,
    /// Input queued during the turn (steers) not yet delivered within it: delivered after its
    /// Stop as a turn of its own, which has no UserPromptSubmit.
    pending_steers: Vec<String>,
    /// When the Session went idle (for a turn that never got its Stop).
    idle_at: Option<Instant>,
    /// The turn's Stop came: `(when, last text, background tasks)`; `turn_end` once the transcript
    /// caught up.
    stop_pending: Option<(Instant, Option<String>, Value)>,
    /// When the Process File appeared without a check-in.
    no_checkin_since: Option<Instant>,
    /// An answer just went back through a hook: no announcing until then.
    answered_until: Option<Instant>,
    /// The engine cannot go on (no plugin, another ccrpc): `run` returns this error.
    fatal: Option<String>,
    holder_id: String,
    shell_pid: u32,
    tty: Option<String>,
    conn: HolderConn,
    out: Out,
    wake: Arc<Wake>,
    fs: FsWatch,
    /// PTY size ccrpc set.
    size: (u16, u16),
    /// Someone took the Pane over: ccrpc only reads from now on.
    taken_over: bool,
    last_size_check: Instant,
    claude_pid: Option<u32>,
    claude_seen_at: Option<Instant>,
    last_locate: Option<Instant>,
    pf: Option<ProcessFile>,
    session_id: Option<String>,
    tailer: Option<Tailer>,
    state: State,
    /// Raw `(status, waitingFor)` last read, with when it was first seen.
    raw_since: Option<((String, Option<String>), Instant)>,
    /// A turn is running (opened by UserPromptSubmit, or running at connect) and has not ended.
    turn_active: bool,
    last_text: Option<String>,
    turn: TurnStats,
    prices: cv_core::pricing::PriceTable,
    library: Library,
    analyzer: Option<Analyzer>,
    /// The analysis in flight: dialog signature and where its result arrives.
    analysis: Option<(String, Receiver<std::result::Result<Layout, String>>)>,
    /// Dialog signatures already analyzed (never twice).
    analyzed: HashSet<String>,
    analyses: VecDeque<Instant>,
    /// Layout of the choice announced, the event, and since when it is off the screen.
    choice_layout: Option<String>,
    choice_event: Option<Value>,
    choice_gone_since: Option<Instant>,
    choice_seq: u64,
    /// The Holder reports size owners (not an older Holder).
    holder_reports_owner: bool,
    /// Request (tool_use id) or dialog announced for the current waiting.
    announced: Option<String>,
    startup_checked: Option<Instant>,
    startup_last: Option<Vec<String>>,
    follow_ups: VecDeque<String>,
    marks: Marks,
    /// Requests that are over (answered here or elsewhere): never matched to a dialog again.
    resolved: HashSet<String>,
    procfile_warned: bool,
    exited: bool,
    /// Events are suppressed (initial settle).
    quiet: bool,
}

impl Engine {
    pub fn open(roots: Roots, holder_id: &str, out: Out, opts: &Opts) -> Result<Engine> {
        let meta = locate::read_holder(&roots, holder_id)?;
        let tty = locate::process_table()?
            .get(&meta.child_pid)
            .and_then(|p| p.tty.clone());
        let (cols, rows) = tty
            .as_deref()
            .and_then(|t| locate::tty_size(t).ok())
            .unwrap_or((80, 24));
        let conn = HolderConn::connect(&roots.holder_socket(holder_id), cols, rows)?;
        let initial = conn.initial_owner(Duration::from_secs(3))?;
        let holder_reports_owner = initial != Owner::Unknown;
        match initial {
            Owner::Other(client) if !opts.force => {
                conn.detach();
                bail!("Session 正被 {client} 使用，加 --force 强制连接");
            }
            Owner::Unknown => eprintln!(
                "ccrpc: Holder {holder_id} 不报告尺寸所有者，无法确认是否有人在用"
            ),
            _ => {}
        }
        conn.claim_size(opts.size.0, opts.size.1)?;
        let wake = Wake::new();
        conn.set_wake(wake.clone());
        let fs = FsWatch::new(wake.clone());
        Ok(Engine {
            roots,
            state_dir: hook::state_dir(),
            hooks: None,
            pre_tools: VecDeque::new(),
            requests: Vec::new(),
            hook_prompts: VecDeque::new(),
            hook_seq: 0,
            hook_tasks: Vec::new(),
            pending_steers: Vec::new(),
            idle_at: None,
            stop_pending: None,
            no_checkin_since: None,
            answered_until: None,
            fatal: None,
            holder_id: holder_id.to_string(),
            shell_pid: meta.child_pid,
            tty,
            conn,
            out,
            wake,
            fs,
            size: opts.size,
            taken_over: false,
            last_size_check: Instant::now(),
            claude_pid: None,
            claude_seen_at: None,
            last_locate: None,
            pf: None,
            session_id: None,
            tailer: None,
            state: State::Starting,
            raw_since: None,
            turn_active: false,
            last_text: None,
            turn: TurnStats::default(),
            prices: cv_core::pricing::PriceTable::embedded(),
            library: Library::load(layout::library_dir()),
            analyzer: Analyzer::from_env(),
            analysis: None,
            analyzed: HashSet::new(),
            analyses: VecDeque::new(),
            choice_layout: None,
            choice_event: None,
            choice_gone_since: None,
            choice_seq: 0,
            holder_reports_owner,
            announced: None,
            startup_checked: None,
            startup_last: None,
            follow_ups: VecDeque::new(),
            marks: Marks::default(),
            resolved: HashSet::new(),
            procfile_warned: false,
            exited: false,
            quiet: false,
        })
    }

    fn emit(&self, v: Value) {
        if !self.quiet {
            self.out.send(&v);
        }
    }

    // ---- observation -------------------------------------------------------------------------

    fn locate_claude(&mut self) {
        if self.claude_pid.is_some() {
            return;
        }
        if self
            .last_locate
            .is_some_and(|t| t.elapsed() < Duration::from_millis(400))
        {
            return;
        }
        self.last_locate = Some(Instant::now());
        if let Ok(table) = locate::process_table()
            && let Some(p) = locate::find_claude_under(&table, self.shell_pid)
        {
            self.claude_pid = Some(p.pid);
            self.claude_seen_at = Some(Instant::now());
            // Listen before reading the check-in: a SessionStart in between still reaches us.
            match HookServer::bind(&self.state_dir, p.pid, self.wake.clone()) {
                Ok(h) => self.hooks = Some(h),
                Err(e) => self.fatal = Some(format!("{e:#}")),
            }
        }
    }

    /// Follows the plugin's check-in to the Session's transcript until one is found.
    fn follow_checkin(&mut self) {
        if self.tailer.is_some() {
            return;
        }
        let Some(pid) = self.claude_pid else { return };
        let Some(c) = hook::read_checkin(&self.state_dir, pid) else {
            // Startup screens come before the Process File (and SessionStart may come after
            // them): only a Process File without a check-in means no plugin.
            if self.pf.is_some() {
                let since = *self.no_checkin_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= CHECKIN_GRACE && self.fatal.is_none() {
                    self.fatal = hook::checkin_problem(None, None);
                }
            }
            return;
        };
        // A check-in left by an earlier claude with the same pid (another Pane, or another
        // Session): wait for this one's.
        if c.holder_id != self.holder_id
            || self.pf.as_ref().and_then(|p| p.session_id.as_deref()).is_some_and(|s| s != c.session_id)
        {
            let since = *self.no_checkin_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= CHECKIN_GRACE && self.fatal.is_none() {
                self.fatal = hook::checkin_problem(Some(&c), self.pf.as_ref().and_then(|p| p.session_id.as_deref()))
                    .or_else(|| hook::checkin_problem(None, None));
            }
            return;
        }
        if let Some(problem) = hook::checkin_problem(Some(&c), None) {
            self.fatal = Some(problem);
            return;
        }
        let path = PathBuf::from(&c.transcript_path);
        if !locate::is_transcript_path(&self.roots, &path) {
            self.fatal = Some(format!("ccrpc 插件报到的 transcript 不在 Claude Code 的 projects 目录里：{}", path.display()));
            return;
        }
        let history = path.exists();
        self.switch_to(c.session_id, &path, history);
    }

    /// Switches to the Session `sid` whose transcript is `path`. A transcript that already existed
    /// is read once as history: its blocks are not emitted, but prompts / enqueues still confirm
    /// commands and pending tools are known. One created afterwards (a new Session's first prompt,
    /// after /clear) is live and read by `tick`.
    fn switch_to(&mut self, sid: String, path: &Path, history: bool) {
        if self.session_id.as_deref() == Some(sid.as_str()) && self.tailer.is_some() {
            return;
        }
        let prev = self.session_id.replace(sid.clone());
        if let Some(prev) = prev
            && prev != sid
        {
            self.emit(json!({"type": "session_changed", "session_id": sid, "previous": prev}));
        }
        let mut t = Tailer::new(path);
        if history {
            loop {
                for ev in t.poll() {
                    match ev {
                        TxEvent::UserPrompt { uuid, text } => self.marks.prompt(uuid, text),
                        TxEvent::Enqueued { content } => self.marks.enqueue(content),
                        TxEvent::Block(b) if b.agent_id.is_none() && b.kind == BlockKind::Text => {
                            self.last_text = b.text.clone();
                        }
                        _ => {}
                    }
                }
                if !t.behind() {
                    break;
                }
            }
        }
        self.tailer = Some(t);
    }

    /// One observation step: process, Process File, transcript, state transitions, events.
    pub fn tick(&mut self) {
        if self.exited {
            return;
        }
        if self.conn.poll_closed() {
            self.set_exited();
            return;
        }
        self.check_takeover();
        self.locate_claude();
        if let Some(pid) = self.claude_pid {
            if !crate::procs::pid_exists(pid) {
                self.set_exited();
                return;
            }
            if let Some(pf) = procfile::read(&self.roots, pid) {
                self.pf = Some(pf);
            }
        }
        self.follow_checkin();
        self.update_watches();
        let evs = self.tailer.as_mut().map(|t| t.poll()).unwrap_or_default();
        for ev in evs {
            match ev {
                TxEvent::Block(b) => {
                    if b.agent_id.is_none() && b.kind == BlockKind::Text {
                        self.last_text = b.text.clone();
                    }
                    if b.agent_id.is_none()
                        && b.kind == BlockKind::User
                        && b.text
                            .as_deref()
                            .is_some_and(|t| t.starts_with("[Request interrupted by user"))
                    {
                        self.turn.aborted = true;
                    }
                    // A steer delivered within the turn.
                    if b.agent_id.is_none() && b.kind == BlockKind::User && b.queued {
                        let t = b.text.as_deref().unwrap_or_default().trim().to_string();
                        self.pending_steers.retain(|s| s.trim() != t);
                    }
                    let mut v = serde_json::to_value(&b).unwrap_or(Value::Null);
                    if let Value::Object(m) = &mut v {
                        m.insert("type".into(), json!("block"));
                    }
                    self.emit(v);
                }
                // Turns are delimited by the plugin's UserPromptSubmit / Stop; the marker says the
                // transcript has the turn's last message.
                TxEvent::TurnEnd { duration_ms } => {
                    // The first marker is the turn's (a steer's own turn may follow in the same read).
                    if self.turn_active && !self.turn.marker_seen {
                        self.turn.marker_seen = true;
                        if duration_ms.is_some() {
                            self.turn.duration_ms = duration_ms;
                        }
                    }
                }
                TxEvent::Usage {
                    model,
                    usage,
                    logged_cost,
                } => {
                    if self.turn_active {
                        let t = &mut self.turn;
                        t.input += usage.input;
                        t.output += usage.output;
                        t.cache_read += usage.cache_read;
                        t.cache_creation += usage.cache_write_5m + usage.cache_write_1h;
                        let c = self.prices.cost(model.as_deref(), &usage, logged_cost);
                        t.cost = t.cost.zip(c).map(|(a, b)| a + b);
                    }
                }
                TxEvent::ApiError { kind, message } => {
                    if self.turn_active && self.turn.error.is_none() {
                        self.turn.error = Some(json!({"kind": kind, "message": message}));
                    }
                }
                TxEvent::UserPrompt { uuid, text } => self.marks.prompt(uuid, text),
                TxEvent::Enqueued { content } => self.marks.enqueue(content),
                TxEvent::Branch { at_uuid } => {
                    self.emit(json!({"type": "branch", "at_uuid": at_uuid}));
                }
                TxEvent::Workflow(v) => self.emit(v),
            }
        }
        while let Some(m) = self.hooks.as_ref().and_then(HookServer::try_recv) {
            self.on_hook(m);
        }
        // A held hook Claude Code killed (answered in the dialog, timed out): the request, if
        // still on screen, is answered with keys from now on.
        for r in &mut self.requests {
            if r.reply.as_ref().is_some_and(hook::peer_gone) {
                r.reply = None;
            }
        }
        if self.answered_until.is_some_and(|t| Instant::now() >= t) {
            self.answered_until = None;
        }
        self.update_state();
        if let Some((at, _, _)) = &self.stop_pending
            && (self.turn.marker_seen || at.elapsed() >= STOP_CATCH_UP)
        {
            self.finish_stopped_turn();
        }
        // Stop comes before idle: a turn that went idle without one was interrupted.
        if self.turn_active
            && self.stop_pending.is_none()
            && self.state == State::Idle
            && self.idle_at.is_some_and(|t| t.elapsed() >= ABORT_GRACE)
        {
            if self.turn.provisional {
                // The steer had been delivered within the previous turn after all.
                self.turn_active = false;
                self.turn = TurnStats::default();
            } else {
                self.turn.aborted = true;
                self.emit_turn_end(None, Value::Null);
            }
        }
        self.refresh_announcement();
        self.announce_requests();
    }

    // ---- hook events -------------------------------------------------------------------------

    fn on_hook(&mut self, m: HookMsg) {
        let i = &m.input;
        let s = |k: &str| i[k].as_str().map(str::to_string);
        match m.event.as_str() {
            "SessionStart" => {
                if let (Some(sid), Some(path)) = (s("session_id"), s("transcript_path")) {
                    let path = PathBuf::from(path);
                    if locate::is_transcript_path(&self.roots, &path) {
                        let history = path.exists();
                        self.switch_to(sid, &path, history);
                    }
                }
            }
            "UserPromptSubmit" => {
                let pid = s("prompt_id").unwrap_or_default();
                let text = s("prompt").unwrap_or_default();
                self.hook_seq += 1;
                self.hook_prompts.push_back((self.hook_seq, pid.clone(), text.clone()));
                if self.hook_prompts.len() > 64 {
                    self.hook_prompts.pop_front();
                }
                if let Some((id, status)) = transcript::task_notification(&text) {
                    self.set_task_status(&id, &status);
                }
                let notice = text.trim_start().starts_with("<task-notification>");
                self.finish_stopped_turn();
                // A steer's own turn has no UserPromptSubmit: one now is a new prompt.
                if self.turn_active && self.turn.provisional {
                    self.turn_active = false;
                    self.turn = TurnStats::default();
                }
                if self.turn_active {
                    match &self.turn.prompt_id {
                        // Input while busy is delivered within the same turn (steer), or after
                        // its Stop as a turn of its own.
                        Some(p) if *p == pid => {
                            if !notice {
                                self.pending_steers.push(text);
                            }
                            return;
                        }
                        // A turn running at connect: a prompt during it is a steer of it.
                        None => {
                            self.turn.prompt_id = Some(pid);
                            if !notice {
                                self.pending_steers.push(text);
                            }
                            return;
                        }
                        // The previous turn never got its Stop.
                        Some(_) => {
                            self.turn.aborted = true;
                            self.emit_turn_end(None, Value::Null);
                        }
                    }
                }
                let trigger = if notice { "task_notification" } else { "prompt" };
                self.pending_steers.clear();
                self.turn = TurnStats::start(Some(pid), trigger);
                self.turn_active = true;
                self.last_text = None;
                // Idle until now is not this turn's: count afresh.
                if self.idle_at.is_some() {
                    self.idle_at = Some(Instant::now());
                }
            }
            "Stop" | "StopFailure" => {
                let pid = s("prompt_id");
                // Another turn's Stop while the previous one still waits for the transcript: the
                // previous one is over (a steer may have opened this one).
                if self
                    .stop_pending
                    .is_some()
                    && pid.is_some()
                    && self.turn.prompt_id != pid
                {
                    self.finish_stopped_turn();
                }
                let ours = self.turn_active
                    && self.stop_pending.is_none()
                    && (self.turn.prompt_id.is_none() || self.turn.prompt_id == pid);
                // A `!` command or a turn ccrpc did not see start.
                if !ours {
                    return;
                }
                // A turn opened without its prompt id (running at connect, a steer delivered
                // after Stop) takes the Stop's.
                if self.turn.prompt_id.is_none() {
                    self.turn.prompt_id = pid.clone();
                }
                if m.event == "StopFailure" {
                    self.turn.error = Some(json!({"kind": s("error"), "message": s("last_assistant_message")}));
                } else if let Some(t) = s("last_assistant_message") {
                    self.last_text = Some(t);
                }
                let bg = i.get("background_tasks").cloned().unwrap_or(json!([]));
                self.note_running_tasks(&bg);
                let last = if m.event == "Stop" { self.last_text.clone() } else { None };
                self.stop_pending = Some((Instant::now(), last, bg));
            }
            "PreToolUse" => {
                // A turn opened without UserPromptSubmit (a steer delivered after Stop) learns
                // its prompt id here.
                if self.turn_active && self.turn.prompt_id.is_none() && i["agent_id"].is_null() {
                    self.turn.prompt_id = s("prompt_id");
                    self.turn.provisional = false;
                }
                if let Some(id) = s("tool_use_id") {
                    self.pre_tools.push_back(PreTool {
                        id,
                        name: s("tool_name").unwrap_or_default(),
                        input: i["tool_input"].clone(),
                        agent_id: s("agent_id"),
                    });
                    if self.pre_tools.len() > 64 {
                        self.pre_tools.pop_front();
                    }
                }
            }
            "PermissionRequest" => self.on_permission_request(m),
            "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => {
                if let Some(id) = s("tool_use_id") {
                    if m.event == "PostToolUse" {
                        self.note_background_launch(&id, i);
                    }
                    self.resolve(&id);
                }
            }
            "SubagentStart" | "SubagentStop" => {
                // Its Subagent Run file appears / is complete: look for it now.
                if let Some(t) = self.tailer.as_mut() {
                    t.rediscover();
                }
                if m.event == "SubagentStop"
                    && let Some(id) = s("agent_id")
                {
                    // Finished on its own (a stopped one gets no SubagentStop); the notice that
                    // follows says how.
                    if self.hook_tasks.iter().any(|t| t.id == id && t.status == "running") {
                        self.set_task_status(&id, "completed");
                    }
                }
            }
            _ => {}
        }
    }

    /// A permission request (or AskUserQuestion) from the plugin. Held for the caller only when
    /// Claude Code shows its own dialog meanwhile (ADR-0003): a background Subagent's request is
    /// not shown until the hook returns, and ExitPlanMode cannot be allowed by a hook. Those are
    /// answered on the screen instead.
    fn on_permission_request(&mut self, m: HookMsg) {
        let i = &m.input;
        let name = i["tool_name"].as_str().unwrap_or_default().to_string();
        let input = i["tool_input"].clone();
        let agent_id = i["agent_id"].as_str().map(str::to_string);
        let taken: HashSet<&str> = self.requests.iter().map(|r| r.id.as_str()).collect();
        let id = self
            .pre_tools
            .iter()
            .rev()
            .find(|p| {
                p.name == name
                    && p.input == input
                    && p.agent_id == agent_id
                    && !taken.contains(p.id.as_str())
                    && !self.resolved.contains(&p.id)
            })
            .map(|p| p.id.clone());
        let hold = id.is_some()
            && agent_id.is_none()
            && !CHOICE_TOOLS.contains(&name.as_str())
            && !self.taken_over;
        let mut reply = m.reply;
        if !hold && let Some(r) = reply.take() {
            let _ = hook::send_reply(r, None);
        }
        let Some(id) = id else { return };
        self.requests.push(HookRequest {
            id,
            tool_name: name,
            input,
            agent_id,
            at: Instant::now(),
            reply,
        });
    }

    /// The request is over: its hook (if still waiting) is released with no decision.
    fn resolve(&mut self, id: &str) {
        self.requests.retain(|r| r.id != id);
        if self.resolved.len() > 4096 {
            self.resolved.clear();
        }
        self.resolved.insert(id.to_string());
        if self.announced.as_deref() == Some(id) {
            self.announced = None;
        }
    }

    fn open_request(&self, id: &str) -> Option<&HookRequest> {
        self.requests.iter().find(|r| r.id == id)
    }

    /// Releases every waiting hook (someone took over: the person answers in the TUI).
    fn release_hooks(&mut self) {
        for r in &mut self.requests {
            r.reply = None;
        }
    }

    /// File changes that matter wake the loop: Process Files, and the transcript with its
    /// Subagent / Workflow / tool-results files.
    fn update_watches(&mut self) {
        use notify::RecursiveMode::{NonRecursive, Recursive};
        let sessions = self.roots.sessions_dir();
        let main = self.tailer.as_ref().map(|t| t.main_path().to_path_buf());
        let mut paths = vec![(sessions.as_path(), NonRecursive)];
        let (dir, sdir);
        if let Some(m) = &main {
            dir = m.parent().map(|p| p.to_path_buf());
            sdir = m.with_extension("");
            if let Some(d) = &dir {
                paths.push((d.as_path(), NonRecursive));
            }
            paths.push((sdir.as_path(), Recursive));
        }
        self.fs.set(&paths);
    }

    /// `turn_end` of the turn whose Stop came, if any.
    fn finish_stopped_turn(&mut self) {
        if let Some((_, last, bg)) = self.stop_pending.take() {
            self.emit_turn_end(last, bg);
            // Input queued in it but not delivered within it runs next, with no
            // UserPromptSubmit: open that turn now (its Stop ends it).
            if !self.pending_steers.is_empty() {
                self.pending_steers.clear();
                self.turn = TurnStats::start(None, "prompt");
                self.turn.provisional = true;
                self.turn_active = true;
                self.last_text = None;
                if self.idle_at.is_some() {
                    self.idle_at = Some(Instant::now());
                }
            }
        }
    }

    /// A background task the plugin saw start: Bash `run_in_background`, an async Agent, a
    /// Workflow (from the tool's response).
    fn note_background_launch(&mut self, tool_use_id: &str, i: &Value) {
        let (resp, input) = (&i["tool_response"], &i["tool_input"]);
        let text = |v: &Value| v.as_str().map(str::to_string);
        let task = match i["tool_name"].as_str() {
            Some("Bash") => text(&resp["backgroundTaskId"]).map(|id| (id, "shell")),
            Some("Agent" | "Task") if resp["isAsync"] == true => text(&resp["agentId"]).map(|id| (id, "agent")),
            Some("Workflow") => text(&resp["taskId"]).map(|id| (id, "workflow")),
            _ => None,
        };
        let Some((id, kind)) = task else { return };
        if self.hook_tasks.iter().any(|t| t.id == id) {
            return;
        }
        let description = text(&input["description"])
            .or_else(|| text(&resp["description"]))
            .or_else(|| text(&resp["summary"]))
            .or_else(|| text(&resp["workflowName"]));
        if self.hook_tasks.len() >= 256 {
            self.hook_tasks.remove(0);
        }
        self.hook_tasks.push(BgTask {
            id,
            kind,
            tool_use_id: tool_use_id.to_string(),
            description,
            command: (kind == "shell").then(|| text(&input["command"])).flatten(),
            status: "running".into(),
        });
    }

    /// Stop's `background_tasks`: running tasks ccrpc did not see start are added.
    fn note_running_tasks(&mut self, bg: &Value) {
        for t in bg.as_array().into_iter().flatten() {
            let Some(id) = t["id"].as_str() else { continue };
            if self.hook_tasks.iter().any(|h| h.id == id)
                || self.tailer.as_ref().is_some_and(|tl| tl.tasks().iter().any(|h| h.id == id))
            {
                continue;
            }
            let kind = match t["type"].as_str() {
                Some("subagent") => "agent",
                Some("shell") => "shell",
                Some("workflow") => "workflow",
                _ => continue,
            };
            self.hook_tasks.push(BgTask {
                id: id.to_string(),
                kind,
                tool_use_id: String::new(),
                description: t["description"].as_str().map(str::to_string),
                command: t["command"].as_str().map(str::to_string),
                status: t["status"].as_str().unwrap_or("running").to_string(),
            });
        }
    }

    fn set_task_status(&mut self, id: &str, status: &str) {
        match self.hook_tasks.iter_mut().find(|t| t.id == id) {
            Some(t) => t.status = status.to_string(),
            None => {
                // One ccrpc saw only in the transcript (started before it connected).
                if let Some(mut t) = self
                    .tailer
                    .as_ref()
                    .and_then(|tl| tl.tasks().iter().find(|t| t.id == id).cloned())
                {
                    t.status = status.to_string();
                    self.hook_tasks.push(t);
                }
            }
        }
    }

    /// Background tasks: what the plugin reported, over what the transcript recorded (tasks
    /// started before ccrpc connected).
    fn tasks(&self) -> Vec<BgTask> {
        let mut out: Vec<BgTask> = self
            .tailer
            .as_ref()
            .map(|t| t.tasks().to_vec())
            .unwrap_or_default();
        for h in &self.hook_tasks {
            match out.iter_mut().find(|t| t.id == h.id) {
                Some(t) => {
                    t.status = h.status.clone();
                    if t.tool_use_id.is_empty() {
                        t.tool_use_id = h.tool_use_id.clone();
                    }
                }
                None => out.push(h.clone()),
            }
        }
        out
    }

    fn emit_turn_end(&mut self, last_text: Option<String>, background_tasks: Value) {
        self.turn_active = false;
        let mut v = self.turn.to_json();
        v["type"] = json!("turn_end");
        v["last_text"] = json!(last_text);
        v["background_tasks"] = match background_tasks {
            Value::Array(a) => Value::Array(a),
            _ => json!([]),
        };
        self.emit(v);
        self.turn = TurnStats::default();
    }

    fn set_exited(&mut self) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.set_state(State::Exited);
        self.emit(json!({"type": "session_exit"}));
    }

    fn set_state(&mut self, s: State) {
        if self.state == s {
            return;
        }
        // Leaving the waiting does not end a request (the Process File may flicker): the plugin's
        // PostToolUse / PermissionDenied or going idle does.
        if matches!(
            self.state,
            State::Waiting {
                kind: WaitKind::Permission | WaitKind::Question,
                ..
            }
        ) && let Some(id) = self.announced.take()
            && self.open_request(&id).is_none()
        {
            self.resolved.insert(id);
        }
        self.announced = None;
        // Nothing of the main line waits for an answer once the Session is idle (an Escape kills
        // waiting hooks). A background Subagent's request may show its dialog only now.
        if s == State::Idle {
            self.idle_at = Some(Instant::now());
            // A background one whose tool is no longer pending (its Subagent was stopped) is over
            // too; a new one may not have its tool_use in the transcript yet.
            let pending = |id: &str| {
                self.tailer
                    .as_ref()
                    .is_some_and(|t| t.pending().iter().any(|p| p.id == id))
            };
            let ids: Vec<String> = self
                .requests
                .iter()
                .filter(|r| {
                    r.agent_id.is_none()
                        || r.reply.is_some()
                        || (r.at.elapsed() >= Duration::from_secs(3) && !pending(&r.id))
                })
                .map(|r| r.id.clone())
                .collect();
            for id in ids {
                self.resolve(&id);
            }
        } else {
            self.idle_at = None;
        }
        self.state = s;
        let mut v = self.state.to_json();
        v["type"] = json!("state");
        self.emit(v);
    }

    fn update_state(&mut self) {
        let Some(pf) = self.pf.clone() else {
            self.report_startup_screen();
            if !self.procfile_warned
                && self
                    .claude_seen_at
                    .is_some_and(|t| t.elapsed() >= Duration::from_secs(20))
            {
                self.procfile_warned = true;
                eprintln!(
                    "ccrpc: claude 已运行 20 秒，仍没有它的进程文件 {}/sessions/<pid>.json；若不是停在启动界面，可能是配置目录不对，用 --claude-config-dir 指定",
                    self.roots.claude_config.display()
                );
            }
            return;
        };
        let status = pf.status.clone().unwrap_or_default();
        let raw = (status.clone(), pf.waiting_for.clone());
        let since = match &self.raw_since {
            Some((r, t)) if *r == raw => *t,
            _ => {
                let now = Instant::now();
                self.raw_since = Some((raw.clone(), now));
                now
            }
        };
        let next = match status.as_str() {
            "busy" => State::Busy,
            "waiting" => {
                let wf = pf.waiting_for.clone().unwrap_or_default();
                let kind = match wf.as_str() {
                    "permission prompt" => WaitKind::Permission,
                    "input needed" => {
                        let has_ask = self.requests.iter().any(|r| r.tool_name == "AskUserQuestion");
                        if has_ask {
                            WaitKind::Question
                        } else if since.elapsed() < CLASSIFY_GRACE || self.answered_until.is_some() {
                            return; // the plugin's PermissionRequest may still be on its way
                        } else {
                            WaitKind::Dialog
                        }
                    }
                    _ => WaitKind::Dialog,
                };
                State::Waiting {
                    kind,
                    waiting_for: wf,
                }
            }
            _ => State::Idle,
        };
        self.set_state(next);
    }

    /// Startup screens (onboarding, folder trust) come before the Process File: report each
    /// distinct one, checking at most once a second.
    fn report_startup_screen(&mut self) {
        let Some(seen) = self.claude_seen_at else {
            return;
        };
        if seen.elapsed() < STARTUP_DIALOG_AFTER
            || self
                .startup_checked
                .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.startup_checked = Some(Instant::now());
        let lines = self.screen().map(|s| s.lines).unwrap_or_default();
        if self.startup_last.as_ref() != Some(&lines) {
            self.startup_last = Some(lines.clone());
            self.emit(json!({"type": "dialog", "waiting_for": "startup", "lines": lines}));
        }
    }

    /// An announced request whose tool got its result is over even if the Process File still says
    /// waiting (two prompts in a row can collapse into one waiting period): announce afresh.
    fn refresh_announcement(&mut self) {
        let Some(id) = &self.announced else {
            return;
        };
        if !matches!(
            self.state,
            State::Waiting {
                kind: WaitKind::Permission | WaitKind::Question,
                ..
            }
        ) {
            return;
        }
        if id.starts_with("dialog:") {
            return;
        }
        // A choice is over once its tool got a result, or its layout has been off the screen a
        // moment (two waits can merge into one waiting period): announce afresh.
        if id.starts_with("choice:") {
            let tool_done = self
                .choice_event
                .as_ref()
                .and_then(|v| v["tool_use_id"].as_str())
                .is_some_and(|tid| self.resolved.contains(tid));
            let shown = self.choice_layout.as_ref().is_some_and(|lid| {
                self.screen()
                    .ok()
                    .and_then(|s| self.library.find_kind(&s.lines, LayoutKind::Choice).map(|(l, _)| &l.id == lid))
                    .unwrap_or(false)
            });
            if shown {
                self.choice_gone_since = None;
            } else if self.choice_gone_since.is_none() {
                self.choice_gone_since = Some(Instant::now());
            }
            let gone = self
                .choice_gone_since
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(250));
            if tool_done || gone {
                self.announced = None;
                self.choice_layout = None;
                self.choice_event = None;
                self.choice_gone_since = None;
                self.raw_since = Some((
                    self.raw_since.take().map(|r| r.0).unwrap_or_default(),
                    Instant::now(),
                ));
            }
            return;
        }
        if self.open_request(id).is_none() {
            if let Some(done) = self.announced.take() {
                self.resolved.insert(done);
            }
            self.raw_since = Some((
                self.raw_since.take().map(|r| r.0).unwrap_or_default(),
                Instant::now(),
            ));
        }
    }

    /// The pending tool the permission dialog on `s` is about: the one whose command / file shows
    /// in the dialog body; failing that, the only candidate when no candidate has anything to show.
    /// A permission dialog on `lines`: the built-in recognizer, else a permission layout from the
    /// library. With the dialog's body for matching the pending tool.
    fn dialog_of(&self, lines: &[String]) -> Option<(PermissionDialog, String)> {
        if let Some(d) = recognize::permission(lines) {
            return Some((d, recognize::permission_body(lines)?));
        }
        let (l, opts) = self.library.find_kind(lines, LayoutKind::Permission)?;
        let options = opts
            .into_iter()
            .map(|o| {
                let c = match o.meaning {
                    Meaning::Allow => PermissionChoice::Allow,
                    Meaning::AllowAlways => PermissionChoice::AllowAlways,
                    Meaning::Deny => PermissionChoice::Deny,
                    Meaning::Other => PermissionChoice::Other,
                };
                (o.number, o.label, c)
            })
            .collect();
        Some((PermissionDialog { options }, l.body(lines)?))
    }

    /// The request the permission dialog on `s` is about, among those answered on the screen
    /// (a background Subagent's): the one whose command / file shows in the dialog body; failing
    /// that, the only candidate when no candidate has anything to show.
    fn screen_request(&self, s: &Screen) -> Option<String> {
        let (_, body) = self.dialog_of(&s.lines)?;
        let cands: Vec<&HookRequest> = self
            .requests
            .iter()
            .filter(|r| r.is_approval() && r.reply.is_none())
            .collect();
        let matched: Vec<&&HookRequest> = cands
            .iter()
            .filter(|r| dialog_needle(&r.tool_name, &r.input).is_some_and(|n| body.contains(&n)))
            .collect();
        // Exactly one request may match; two that both fit the dialog are ambiguous.
        match matched.len() {
            1 => return Some(matched[0].id.clone()),
            0 => {}
            _ => return None,
        }
        let blind: Vec<&&HookRequest> = cands
            .iter()
            .filter(|r| dialog_needle(&r.tool_name, &r.input).is_none())
            .collect();
        (blind.len() == 1 && cands.len() == 1).then(|| blind[0].id.clone())
    }

    /// A permission dialog the plugin never reported (it was up before ccrpc connected): taken
    /// from the transcript's pending tool whose command / file shows in it, answered with keys.
    fn adopt_pending(&mut self, s: &Screen) {
        if self.requests.iter().any(|r| r.is_approval())
            || self.waiting_age() < Duration::from_millis(250)
        {
            return;
        }
        let Some((_, body)) = self.dialog_of(&s.lines) else {
            return;
        };
        let Some(t) = &self.tailer else { return };
        let cands: Vec<_> = t
            .pending()
            .into_iter()
            .filter(|p| !NO_PERMISSION_TOOLS.contains(&p.name.as_str()))
            .filter(|p| !CHOICE_TOOLS.contains(&p.name.as_str()))
            .filter(|p| !self.resolved.contains(&p.id))
            // A tool the plugin reported at PreToolUse gets its own PermissionRequest.
            .filter(|p| !self.pre_tools.iter().any(|t| t.id == p.id))
            .collect();
        let matched: Vec<_> = cands
            .iter()
            .filter(|p| dialog_needle(&p.name, &p.input).is_some_and(|n| body.contains(&n)))
            .collect();
        let pick = match (matched.as_slice(), cands.as_slice()) {
            ([p], _) => *p,
            ([], [p]) if dialog_needle(&p.name, &p.input).is_none() => p,
            _ => return,
        };
        self.requests.push(HookRequest {
            id: pick.id.clone(),
            tool_name: pick.name.clone(),
            input: pick.input.clone(),
            agent_id: pick.agent_id.clone(),
            at: Instant::now(),
            reply: None,
        });
    }

    fn permission_event(r: &HookRequest) -> Value {
        json!({"type": "permission_request", "request_id": r.id, "tool_name": r.tool_name,
            "input": r.input, "agent_id": r.agent_id})
    }

    fn waiting_age(&self) -> Duration {
        self.raw_since
            .as_ref()
            .map(|(_, t)| t.elapsed())
            .unwrap_or_default()
    }

    fn announce_requests(&mut self) {
        self.poll_analysis();
        let State::Waiting { kind, waiting_for } = self.state.clone() else {
            return;
        };
        if self.announced.is_some() || self.answered_until.is_some() {
            return;
        }
        match kind {
            WaitKind::Permission => {
                // A main-line request waits in its hook: answered through it, whatever the screen.
                if let Some(r) = self.requests.iter().find(|r| r.is_approval() && r.reply.is_some()) {
                    let ev = Self::permission_event(r);
                    self.announced = Some(r.id.clone());
                    self.emit(ev);
                    return;
                }
                let screen = self.screen().ok();
                if let Some(s) = &screen {
                    self.adopt_pending(s);
                }
                if let Some(id) = screen.as_ref().and_then(|s| self.screen_request(s))
                    && let Some(r) = self.open_request(&id)
                {
                    let ev = Self::permission_event(r);
                    self.announced = Some(id);
                    self.emit(ev);
                    return;
                }
                let Some(s) = screen else {
                    return;
                };
                // Wait for the dialog to render before taking it for something else.
                if self.waiting_age() < Duration::from_millis(250) {
                    return;
                }
                if self.announce_choice(&s) {
                    return;
                }
                if self.waiting_age() >= MATCH_GRACE {
                    if self.analyzing(&s, &waiting_for) {
                        return;
                    }
                    // Cannot tie the dialog to a request: hand the screen to the caller.
                    self.announced = Some(format!("dialog:{waiting_for}"));
                    self.emit(
                        json!({"type": "dialog", "waiting_for": waiting_for, "lines": s.lines}),
                    );
                }
            }
            WaitKind::Question => {
                let Some(r) = self.requests.iter().find(|r| r.tool_name == "AskUserQuestion") else {
                    return;
                };
                let ev = json!({"type": "question", "request_id": r.id,
                    "questions": r.input.get("questions").cloned().unwrap_or(Value::Null), "agent_id": r.agent_id});
                self.announced = Some(r.id.clone());
                self.emit(ev);
            }
            WaitKind::Dialog => {
                // The Process File says waiting before the dialog is drawn: let it render first.
                if self.waiting_age() < Duration::from_millis(250) {
                    return;
                }
                let s = self.screen().unwrap_or_default();
                if self.announce_choice(&s) || self.analyzing(&s, &waiting_for) {
                    return;
                }
                self.announced = Some(format!("dialog:{waiting_for}"));
                self.emit(json!({"type": "dialog", "waiting_for": waiting_for, "lines": s.lines}));
            }
        }
    }

    /// A choice layout on screen: announce it. True when announced.
    fn announce_choice(&mut self, s: &Screen) -> bool {
        let Some((l, opts)) = self.library.find_kind(&s.lines, LayoutKind::Choice) else {
            return false;
        };
        // While a tool waits for permission, only a tool whose approval is a menu (ExitPlanMode)
        // may be answered as a choice: anything else would hide the tool from the caller.
        if let State::Waiting {
            kind: WaitKind::Permission,
            ..
        } = &self.state
        {
            let tools: Vec<&HookRequest> =
                self.requests.iter().filter(|r| r.tool_name != "AskUserQuestion").collect();
            let tools_ok = !tools.is_empty()
                && tools.iter().all(|r| CHOICE_TOOLS.contains(&r.tool_name.as_str()));
            if !tools_ok {
                return false;
            }
        }
        let (id, title) = (l.id.clone(), l.title.clone());
        // A choice raised by a tool (ExitPlanMode…): the single pending tool, if any.
        let tool = match self.requests.as_slice() {
            [r] => Some(r.id.clone()),
            _ => None,
        };
        self.choice_seq += 1;
        let rid = format!("choice:{}", self.choice_seq);
        self.announced = Some(rid.clone());
        self.choice_layout = Some(id.clone());
        self.choice_gone_since = None;
        let options: Vec<Value> = opts
            .iter()
            .map(|o| json!({"label": o.label, "meaning": o.meaning, "description": o.description}))
            .collect();
        let ev = json!({"type": "choice", "request_id": rid, "title": title, "options": options,
            "tool_use_id": tool, "layout": id});
        self.choice_event = Some(ev.clone());
        self.emit(ev);
        true
    }

    /// Hands an unrecognized dialog to the analyzer (once per dialog, rate-limited). True while
    /// its analysis is in flight: the dialog is not reported as unrecognized meanwhile.
    fn analyzing(&mut self, s: &Screen, waiting_for: &str) -> bool {
        // One analysis at a time; meanwhile nothing else is reported for the dialog.
        if self.analysis.is_some() {
            return true;
        }
        // What the built-in recognizers know is never sent out.
        if recognize::permission(&s.lines).is_some() || recognize::ask(&s.lines).is_some() {
            return false;
        }
        let sig = dialog_signature(&s.lines);
        let Some(analyzer) = self.analyzer.clone() else {
            return false;
        };
        if sig.is_empty() || self.analyzed.contains(&sig) {
            return false;
        }
        while self
            .analyses
            .front()
            .is_some_and(|t| t.elapsed() > Duration::from_secs(3600))
        {
            self.analyses.pop_front();
        }
        if self.analyses.len() >= MAX_ANALYSES_PER_HOUR {
            return false;
        }
        self.analyses.push_back(Instant::now());
        self.analyzed.insert(sig.clone());
        // What the dialog may be about: the plugin's requests, else the transcript's pending tools.
        let mut pending: Vec<(String, Value)> = self
            .requests
            .iter()
            .map(|r| (r.tool_name.clone(), r.input.clone()))
            .collect();
        if pending.is_empty() {
            pending = self
                .tailer
                .as_ref()
                .map(|t| {
                    t.pending()
                        .into_iter()
                        .filter(|p| !self.resolved.contains(&p.id))
                        .filter(|p| !NO_PERMISSION_TOOLS.contains(&p.name.as_str()))
                        .map(|p| (p.name.clone(), p.input.clone()))
                        .collect()
                })
                .unwrap_or_default();
        }
        let lines = s.lines.clone();
        let waiting_for = waiting_for.to_string();
        let version = self.pf.as_ref().and_then(|p| p.version.clone());
        let (tx, rx) = mpsc::channel();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let req = analyze::Request {
                lines: &lines,
                waiting_for: &waiting_for,
                pending,
                claude_version: version,
            };
            let _ = tx.send(analyzer.analyze(&req).map_err(|e| format!("{e:#}")));
            wake.bump();
        });
        self.analysis = Some((sig, rx));
        true
    }

    /// Takes the analyzer's answer: a layout that passes its structural checks on the screen it
    /// came from goes into the library (and is used at once, ADR-0002).
    fn poll_analysis(&mut self) {
        let Some((_, rx)) = &self.analysis else {
            return;
        };
        let res = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("分析器线程意外结束".into()),
        };
        self.analysis = None;
        let layout = match res {
            Ok(l) if l.match_screen(&l.screen).is_some() => l,
            Ok(l) => {
                self.emit(json!({"type": "analysis_failed", "error": "分析结果与屏幕对不上", "title": l.title}));
                return;
            }
            Err(e) => {
                self.emit(json!({"type": "analysis_failed", "error": e}));
                return;
            }
        };
        let (id, kind, title) = (layout.id.clone(), layout.kind, layout.title.clone());
        match self.library.save(layout) {
            Ok(path) => self.emit(json!({"type": "layout_learned", "layout": id, "kind": kind,
                "title": title, "path": path.display().to_string()})),
            Err(e) => self.emit(json!({"type": "analysis_failed", "error": format!("{e:#}")})),
        }
    }

    /// Waits until the waiting dialog has been up [`DIALOG_SETTLE`].
    fn settle_dialog(&mut self) {
        let left = DIALOG_SETTLE.saturating_sub(self.waiting_age());
        if !left.is_zero() {
            self.wait_until(left, |_| false);
        }
    }

    /// Answers the announced choice by an option's text (exact, else a unique prefix).
    fn answer_choice(&mut self, request_id: &str, option: &str) -> CmdResult {
        match &self.state {
            State::Waiting { .. } => {}
            State::Exited => return err("exited", "Session 已结束"),
            _ => return err("not_waiting", format!("当前状态为 {}", self.state.name())),
        }
        if self.announced.as_deref() != Some(request_id) || !request_id.starts_with("choice:") {
            return err("unknown_request", format!("当前挂起的选择不是 {request_id}"));
        }
        self.settle_dialog();
        let now = self.fresh_screen()?;
        let Some((l, opts)) = self.library.find_kind(&now.lines, LayoutKind::Choice) else {
            return Err(self.unrecognized("answer_choice", Some(&now)));
        };
        if Some(&l.id) != self.choice_layout.as_ref() {
            return err("unknown_request", "屏幕上已经不是这个选择");
        }
        let exact: Vec<_> = opts.iter().filter(|o| o.label == option).collect();
        let prefix: Vec<_> = opts.iter().filter(|o| o.label.starts_with(option)).collect();
        let pick = match (exact.as_slice(), prefix.as_slice()) {
            ([o], _) | ([], [o]) => o.number,
            _ => {
                let labels: Vec<&str> = opts.iter().map(|o| o.label.as_str()).collect();
                return err("bad_request", format!("没有唯一对应的选项：{option}；可选 {labels:?}"));
            }
        };
        self.keys(pick.to_string().as_bytes())?;
        let rid = request_id.to_string();
        let ok = self.wait_until(Duration::from_secs(10), |e| {
            !matches!(e.state, State::Waiting { .. }) || e.announced.as_deref() != Some(rid.as_str())
        });
        if ok {
            Ok(json!({"state": self.state.name()}))
        } else {
            err("timeout", "作答后 10 秒内状态没有变化")
        }
    }

    /// Sends the next follow-up when the Session is idle with no turn open. Only called from the
    /// top-level loop.
    fn drain_follow_ups(&mut self) {
        if self.state != State::Idle || self.turn_active {
            return;
        }
        let Some(text) = self.follow_ups.pop_front() else {
            return;
        };
        if let Err((code, msg)) = self.send_prompt(&text) {
            self.emit(json!({"type": "follow_up_failed", "text": text, "error": {"code": code, "message": msg}}));
        }
    }

    // ---- screen ------------------------------------------------------------------------------

    pub fn screen(&self) -> Result<Screen> {
        Ok(self.conn.screen())
    }

    /// The screen from a Snapshot taken now (before a key whose meaning depends on the screen).
    fn fresh_screen(&self) -> std::result::Result<Screen, CmdErr> {
        self.conn
            .resync(Duration::from_secs(3))
            .map_err(|e| ("io", format!("{e:#}")))
    }

    /// PTY size from the Pane shell's tty.
    fn pty_size(&self) -> Option<(u16, u16)> {
        locate::tty_size(self.tty.as_deref()?).ok()
    }

    /// A size owner other than ccrpc means someone took the Pane over (a Holder that does not
    /// report owners: the PTY size changed). Afterwards the VT follows the PTY size.
    fn check_takeover(&mut self) {
        if !self.taken_over {
            // Sticky: someone who took the size and left between two looks still took over.
            if let Some(client) = self.conn.taken_by() {
                return self.take_over(Some(client));
            }
            if self.conn.owner() != Owner::Unknown {
                return;
            }
        }
        if self.last_size_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_size_check = Instant::now();
        let Some(size) = self.pty_size() else {
            return;
        };
        if !self.taken_over {
            if size != self.size {
                self.take_over(None);
            }
        } else if size != self.conn.vt_size() {
            let _ = self.conn.follow_size(size.0, size.1);
        }
    }

    fn take_over(&mut self, client: Option<String>) {
        self.taken_over = true;
        self.follow_ups.clear();
        self.release_hooks();
        self.emit(json!({"type": "taken_over", "client": client}));
        if let Some((cols, rows)) = self.pty_size() {
            let _ = self.conn.follow_size(cols, rows);
        }
    }

    /// Ticks until `cond` holds or `timeout` passes.
    fn wait_until(&mut self, timeout: Duration, mut cond: impl FnMut(&Engine) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let seen = self.wake.seen();
            self.tick();
            if cond(self) {
                return true;
            }
            if self.exited || Instant::now() >= deadline {
                return false;
            }
            self.wake.wait(seen, TICK);
        }
    }

    /// Polls the screen until `f` returns `Some`.
    fn wait_screen<T>(
        &mut self,
        timeout: Duration,
        mut f: impl FnMut(&Engine, &Screen) -> Option<T>,
    ) -> Option<(T, Screen)> {
        let deadline = Instant::now() + timeout;
        loop {
            let seen = self.conn.generation();
            self.tick();
            if let Ok(s) = self.screen()
                && let Some(v) = f(self, &s)
            {
                return Some((v, s));
            }
            if self.exited || Instant::now() >= deadline {
                return None;
            }
            self.conn.wait_change(seen, TICK);
        }
    }

    /// Every write goes through here or [`Self::paste`]: none once someone took the Pane over,
    /// even in the middle of a command.
    fn writable(&self) -> std::result::Result<(), CmdErr> {
        // The Holder's report counts at once, before a tick turns it into `taken_over`.
        if self.taken_over || self.conn.taken_by().is_some() {
            return err("taken_over", "Session 已被人接手，只读");
        }
        Ok(())
    }

    fn paste(&self, text: &str) -> std::result::Result<(), CmdErr> {
        self.writable()?;
        self.conn
            .paste(text)
            .map_err(|e| ("io", format!("{e:#}")))
    }

    fn keys(&self, bytes: &[u8]) -> std::result::Result<(), CmdErr> {
        self.writable()?;
        let seen = self.conn.generation();
        self.conn
            .input(bytes)
            .map_err(|e| ("io", format!("{e:#}")))?;
        std::thread::sleep(KEY_MIN_GAP);
        self.conn.wait_change(seen, KEY_SETTLE - KEY_MIN_GAP);
        Ok(())
    }

    fn unrecognized(&self, context: &str, s: Option<&Screen>) -> CmdErr {
        let lines = match s {
            Some(s) => s.lines.clone(),
            None => self.screen().map(|s| s.lines).unwrap_or_default(),
        };
        self.emit(json!({"type": "unrecognized", "context": context, "lines": lines}));
        (
            "unrecognized_screen",
            format!("{context}：屏幕与已知布局不符"),
        )
    }

    /// The prompt input box is on screen (no dialog has it): returns its text.
    fn input_box_now(&self) -> std::result::Result<String, CmdErr> {
        let s = self.screen().map_err(|e| ("io", format!("{e:#}")))?;
        recognize::input_box(&s.lines).ok_or_else(|| self.unrecognized("输入框", Some(&s)))
    }

    /// Empties the prompt input box (`ctrl+u` + backspace per line), checking the screen.
    fn clear_input(&mut self) -> std::result::Result<(), CmdErr> {
        for _ in 0..3 {
            let text = self.input_box_now()?;
            if text.is_empty() {
                return Ok(());
            }
            let n = text.lines().count().max(1);
            let mut b = Vec::new();
            for i in 0..n {
                b.extend_from_slice(b"\x15");
                if i + 1 < n {
                    b.extend_from_slice(b"\x7f");
                }
            }
            self.keys(&b)?;
            self.wait_screen(Duration::from_millis(500), |_, s| {
                recognize::input_box(&s.lines).filter(|t| t.is_empty())
            });
        }
        if self.input_box_now()?.is_empty() {
            return Ok(());
        }
        err("input_not_empty", "输入框清不空")
    }

    /// Pastes `clean` and presses Enter, checking before each step that the input box (not a
    /// dialog) has the screen, so Enter cannot answer a dialog.
    fn paste_and_submit(&mut self, clean: &str) -> std::result::Result<(), CmdErr> {
        let before = self.input_box_now()?;
        self.paste(clean)?;
        // Long pastes show as a `[Pasted text …]` placeholder: wait for the box to change.
        self.wait_screen(Duration::from_secs(2), |_, s| {
            recognize::input_box(&s.lines).filter(|t| !t.is_empty() && *t != before)
        });
        self.input_box_now()?;
        self.keys(b"\r")
    }

    // ---- commands ----------------------------------------------------------------------------

    fn require(&self, ok: bool, code: &'static str) -> std::result::Result<(), CmdErr> {
        if self.state == State::Exited {
            return err("exited", "Session 已结束");
        }
        if ok {
            Ok(())
        } else {
            err(code, format!("当前状态为 {}", self.state.name()))
        }
    }

    fn clean_text(text: &str) -> std::result::Result<String, CmdErr> {
        let clean = pty::sanitize(text);
        if clean.trim().is_empty() {
            return err("bad_request", "去掉控制字符后正文为空");
        }
        Ok(clean)
    }

    fn send_prompt(&mut self, text: &str) -> CmdResult {
        self.send_prompt_with(text, &[])
    }

    fn send_prompt_with(&mut self, text: &str, images: &[String]) -> CmdResult {
        // Idle and no turn open. A turn that went idle without its Stop (interrupted) closes within
        // a few seconds: wait for that.
        if self.state == State::Idle && self.turn_active {
            self.wait_until(Duration::from_secs(6), |e| {
                !e.turn_active || e.state != State::Idle
            });
        }
        self.require(self.state == State::Idle && !self.turn_active, "not_idle")?;
        let clean = Self::clean_text(text)?;
        let mut paths = Vec::new();
        for p in images {
            if p.chars().any(char::is_control) {
                return err("bad_request", format!("图片路径里有控制字符：{p:?}"));
            }
            // Absolute: Claude Code resolves a relative path against its own cwd.
            match std::path::absolute(p) {
                Ok(abs) if abs.is_file() => paths.push(abs.display().to_string()),
                _ => return err("bad_request", format!("图片文件不存在：{p}")),
            }
        }
        let images = &paths;
        self.clear_input()?;
        // Images are pasted by path, as a terminal drop would; each shows as `[Image #n]`.
        for p in images {
            let had = self.input_box_now()?.matches("[Image #").count();
            self.paste(p)?;
            // One more `[Image #n]` placeholder, and the path itself is gone from the box.
            let shown = self.wait_screen(Duration::from_secs(3), |_, s| {
                recognize::input_box(&s.lines)
                    .filter(|t| !t.contains(p.as_str()) && t.matches("[Image #").count() == had + 1)
            });
            if shown.is_none() {
                return err("bad_request", format!("Claude Code 没有把 {p} 当成图片"));
            }
        }
        let before = self.marks.seq;
        let hook_before = self.hook_seq;
        self.paste_and_submit(&clean)?;
        let found = |e: &Engine| -> Option<String> {
            if images.is_empty() {
                e.marks.prompt_after(before, &clean).map(str::to_string)
            } else {
                e.marks
                    .prompt_with_images_after(before, &clean)
                    .map(str::to_string)
            }
        };
        let ok = self.wait_until(Duration::from_secs(30), |e| found(e).is_some());
        if !ok {
            return err("timeout", "30 秒内没在 transcript 里看到这条提示词");
        }
        // The plugin saw it submitted, normally before it reached the transcript.
        let hook_prompt = |e: &Engine| {
            e.hook_prompts
                .iter()
                .rev()
                .find(|(seq, _, t)| *seq > hook_before && t.trim().ends_with(clean.trim()))
                .map(|(_, p, _)| p.clone())
        };
        if hook_prompt(self).is_none() {
            self.wait_until(Duration::from_secs(1), |e| hook_prompt(e).is_some());
        }
        let prompt_id = hook_prompt(self);
        Ok(json!({"uuid": found(self), "prompt_id": prompt_id}))
    }

    fn steer(&mut self, text: &str) -> CmdResult {
        self.require(matches!(self.state, State::Busy), "not_busy")?;
        let clean = Self::clean_text(text)?;
        self.clear_input()?;
        let before = self.marks.seq;
        self.paste_and_submit(&clean)?;
        let mut how = None;
        self.wait_until(Duration::from_secs(10), |e| {
            if e.marks.enqueued_after(before, &clean) {
                how = Some("queued");
            } else if e.marks.prompt_after(before, &clean).is_some() {
                how = Some("prompt");
            }
            how.is_some()
        });
        match how {
            Some(h) => Ok(json!({"delivered_as": h})),
            None => err("timeout", "10 秒内没在 transcript 里看到这条插话"),
        }
    }

    fn abort(&mut self) -> CmdResult {
        self.require(
            matches!(self.state, State::Busy | State::Waiting { .. }),
            "not_busy",
        )?;
        self.keys(b"\x1b")?;
        if self.wait_until(Duration::from_secs(10), |e| {
            !matches!(e.state, State::Busy | State::Waiting { .. })
        }) {
            Ok(json!({"state": self.state.name()}))
        } else {
            err("timeout", "10 秒内没有停下")
        }
    }

    fn request_is(&self, kind: &WaitKind, request_id: &str) -> bool {
        matches!(&self.state, State::Waiting { kind: k, .. } if k == kind)
            && self.announced.as_deref() == Some(request_id)
    }

    fn check_request(&self, kind: WaitKind, request_id: &str) -> std::result::Result<(), CmdErr> {
        match &self.state {
            State::Waiting { kind: k, .. } if *k == kind => {}
            State::Exited => return err("exited", "Session 已结束"),
            _ => return err("not_waiting", format!("当前状态为 {}", self.state.name())),
        }
        if self.announced.as_deref() != Some(request_id) {
            return err(
                "unknown_request",
                format!("当前挂起的请求不是 {request_id}"),
            );
        }
        Ok(())
    }

    fn wait_leave_request(&mut self, request_id: &str) -> CmdResult {
        let rid = request_id.to_string();
        let ok = self.wait_until(Duration::from_secs(10), |e| {
            !matches!(e.state, State::Waiting { .. })
                || e.announced.as_deref().is_some_and(|a| a != rid)
        });
        if ok {
            Ok(json!({"state": self.state.name()}))
        } else {
            err("timeout", "作答后 10 秒内状态没有变化")
        }
    }

    fn answer_permission(
        &mut self,
        request_id: &str,
        decision: &Decision,
        message: Option<&str>,
    ) -> CmdResult {
        self.check_request(WaitKind::Permission, request_id)?;
        let Some(idx) = self.requests.iter().position(|r| r.id == request_id) else {
            return err("unknown_request", format!("当前挂起的请求不是 {request_id}"));
        };
        // A hook that went away while waiting: answer on the screen instead.
        if self.requests[idx].reply.as_ref().is_some_and(hook::peer_gone) {
            self.requests[idx].reply = None;
        }
        // "Always allow" is the dialog's own option: the rule it adds (`python3 *`) is not the
        // suggestion the hook gets (the exact command). The held hook is let go after the key.
        if *decision == Decision::AllowAlways && self.requests[idx].reply.is_some() {
            self.writable()?;
            let held = self.requests[idx].reply.take();
            let res = self.answer_permission_on_screen(request_id, decision, None);
            if res.is_err()
                && let Some(r) = self.requests.iter_mut().find(|r| r.id == request_id)
            {
                r.reply = held;
            }
            return res;
        }
        if self.requests[idx].reply.is_some() {
            self.writable()?;
            let output = match decision {
                Decision::Allow | Decision::AllowAlways => hook::allow_output(None, None),
                Decision::Deny => hook::deny_output(message),
            };
            let stream = self.requests[idx].reply.take().expect("held");
            if hook::send_reply(stream, Some(output)).is_ok() {
                self.answered(request_id);
                return self.wait_leave_request(request_id);
            }
            // The hook was gone after all: the dialog is still there for keys.
        }
        self.answer_permission_on_screen(request_id, decision, message)
    }

    /// The request was answered through its hook: it is over, and the closing dialog is not
    /// taken for a new one.
    fn answered(&mut self, id: &str) {
        self.resolve(id);
        // `announced` stays on the request until the state moves on.
        self.announced = Some(id.to_string());
        self.answered_until = Some(Instant::now() + ANSWER_SETTLE);
    }

    /// Answers a request whose hook is not waiting (a background Subagent's) with the dialog's
    /// option key.
    fn answer_permission_on_screen(
        &mut self,
        request_id: &str,
        decision: &Decision,
        message: Option<&str>,
    ) -> CmdResult {
        let rid = request_id.to_string();
        // The dialog on screen must be the one for this request.
        let found = self.wait_screen(Duration::from_secs(3), |e, s| {
            let (d, _) = e.dialog_of(&s.lines)?;
            (e.screen_request(s)? == rid).then_some(d)
        });
        let Some((d, s)) = found else {
            return Err(self.unrecognized("answer_permission", None));
        };
        let choice = match decision {
            Decision::Allow => PermissionChoice::Allow,
            Decision::AllowAlways => PermissionChoice::AllowAlways,
            Decision::Deny => PermissionChoice::Deny,
        };
        let Some(n) = d.number_for(choice) else {
            if choice == PermissionChoice::AllowAlways {
                return err("not_offered", "这次审批没有“总是允许”选项");
            }
            return Err(self.unrecognized("answer_permission", Some(&s)));
        };
        self.settle_dialog();
        // Last check right before the key: still the same request, on a Snapshot taken now.
        let now = self.fresh_screen()?;
        if !self.request_is(&WaitKind::Permission, request_id)
            || self.screen_request(&now).is_none_or(|id| id != rid)
        {
            return err(
                "unknown_request",
                format!("作答前请求已经变了，不再是 {request_id}"),
            );
        }
        self.keys(n.to_string().as_bytes())?;
        // A key the dialog dropped leaves it there: press once more if it still is this request's.
        let left = |e: &Engine| !e.request_is(&WaitKind::Permission, &rid);
        if !self.wait_until(Duration::from_secs(2), left) {
            let now = self.fresh_screen()?;
            if self.request_is(&WaitKind::Permission, request_id)
                && self.screen_request(&now).is_some_and(|id| id == rid)
            {
                self.keys(n.to_string().as_bytes())?;
            }
        }
        let mut res = self.wait_leave_request(request_id)?;
        // The dialog's No interrupts the turn: the message goes as the next prompt.
        if let Some(msg) = message {
            if !self.wait_until(Duration::from_secs(10), |e| e.state == State::Idle && !e.turn_active) {
                return err("timeout", "拒绝后 10 秒内没有回到 idle，说明没有发出");
            }
            let sent = self.send_prompt(msg)?;
            res["message_uuid"] = sent["uuid"].clone();
        }
        Ok(res)
    }

    /// Answers an AskUserQuestion through its waiting hook (the answers go to the tool as its
    /// input, as if picked in the dialog).
    fn answer_question(&mut self, request_id: &str, answers: &[Answer]) -> CmdResult {
        self.check_request(WaitKind::Question, request_id)?;
        let Some(idx) = self.requests.iter().position(|r| r.id == request_id) else {
            return err("unknown_request", format!("当前挂起的请求不是 {request_id}"));
        };
        if self.requests[idx].reply.as_ref().is_none_or(hook::peer_gone) {
            return err("unsupported", "这次提问没有在 ccrpc 插件里等待作答，只能在屏幕上回答");
        }
        let input = self.requests[idx].input.clone();
        let questions = input
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let picked = question_answers(&questions, answers).map_err(|m| ("bad_request", m))?;
        self.writable()?;
        let mut updated = input;
        updated["answers"] = Value::Object(picked);
        let stream = self.requests[idx].reply.take().expect("held");
        hook::send_reply(stream, Some(hook::allow_output(None, Some(updated))))
            .map_err(|e| ("io", format!("回复 hook 失败：{e}")))?;
        self.answered(request_id);
        self.wait_leave_request(request_id)
    }

    fn slash(&mut self, command: &str) -> CmdResult {
        self.require(self.state == State::Idle, "not_idle")?;
        let clean = Self::clean_text(command)?;
        if clean.contains(['\n', '\t']) {
            return err("bad_request", "斜杠命令必须是单行");
        }
        self.clear_input()?;
        let typed = format!("/{}", clean.trim());
        self.keys(typed.as_bytes())?;
        self.wait_screen(Duration::from_secs(1), |_, s| {
            recognize::input_box(&s.lines).filter(|t| t.starts_with(&typed))
        });
        self.input_box_now()?;
        self.keys(b"\r")?;
        // Submitted once the command left the input box (or a picker replaced the box).
        self.wait_screen(Duration::from_secs(2), |_, s| {
            match recognize::input_box(&s.lines) {
                Some(t) if t.starts_with(&typed) => None,
                _ => Some(()),
            }
        });
        Ok(json!({}))
    }

    fn send_keys(&mut self, keys: &[rpc::Key]) -> CmdResult {
        self.require(true, "")?;
        for k in keys {
            self.keys(&rpc::key_to_bytes(k))?;
        }
        Ok(json!({}))
    }

    fn cancel_follow_up(&mut self, index: Option<usize>) -> CmdResult {
        let removed = match index {
            Some(i) if i < self.follow_ups.len() => {
                self.follow_ups.remove(i);
                1
            }
            Some(i) => {
                return err(
                    "bad_request",
                    format!("follow_up 队列只有 {} 条，没有第 {i} 条", self.follow_ups.len()),
                );
            }
            None => std::mem::take(&mut self.follow_ups).len(),
        };
        Ok(json!({"removed": removed, "follow_ups": self.follow_ups}))
    }

    /// Takes the Session back after a takeover: without `force` only once the person's client
    /// is gone (no size owner online).
    fn reclaim(&mut self, force: bool) -> CmdResult {
        if self.state == State::Exited {
            return err("exited", "Session 已结束");
        }
        if !self.taken_over {
            return err("not_taken_over", "Session 没有被接手");
        }
        match self.conn.owner() {
            Owner::Me | Owner::Nobody => {}
            Owner::Other(c) if !force => {
                return err("taken_over", format!("Session 正被 {c} 使用，加 force 强制取回"));
            }
            Owner::Unknown if !force => {
                return err(
                    "taken_over",
                    "Holder 不报告尺寸所有者，无法确认对方已离开，加 force 强制取回",
                );
            }
            _ => {}
        }
        self.conn.clear_taken_by();
        self.conn
            .claim_size(self.size.0, self.size.1)
            .map_err(|e| ("io", format!("{e:#}")))?;
        self.taken_over = false;
        self.last_size_check = Instant::now();
        self.emit(json!({"type": "reclaimed"}));
        Ok(json!({"size": [self.size.0, self.size.1]}))
    }

    fn mode_now(&self) -> Option<rpc::PermissionMode> {
        self.screen().ok().and_then(|s| recognize::permission_mode(&s.lines))
    }

    /// Cycles the permission mode with shift+tab until the footer shows `mode`.
    fn set_mode(&mut self, mode: rpc::PermissionMode) -> CmdResult {
        // Idle only: on the way the cycle may pass through auto (or bypass) mode.
        self.require(self.state == State::Idle, "not_idle")?;
        self.input_box_now()?;
        let Some(start) = self.mode_now() else {
            return Err(self.unrecognized("set_mode", None));
        };
        let mut cur = start;
        for _ in 0..=rpc::PermissionMode::CYCLE.len() {
            if cur == mode {
                return Ok(json!({"mode": mode.name()}));
            }
            self.keys(b"\x1b[Z")?;
            let was = cur;
            let next = self.wait_screen(Duration::from_secs(2), |_, s| {
                recognize::permission_mode(&s.lines).filter(|m| *m != was)
            });
            let Some((m, _)) = next else {
                let now = self.mode_now().map_or("未知", |m| m.name());
                self.unrecognized("set_mode", None);
                return err("unrecognized_screen", format!("set_mode：模式行没有变化，当前为 {now}"));
            };
            cur = m;
            // Never stop on bypass permissions: it is passed through, not a target.
            if cur == start {
                break;
            }
        }
        if cur == rpc::PermissionMode::Bypass {
            self.unrecognized("set_mode", None);
            return err("unrecognized_screen", "set_mode：停在了 bypass permissions，当前为 bypass");
        }
        if cur == mode {
            return Ok(json!({"mode": mode.name()}));
        }
        err("not_offered", format!("权限模式里没有 {}", mode.name()))
    }

    /// Stops a running background task from Claude Code's `/tasks` panel.
    fn stop_task(&mut self, id: &str) -> CmdResult {
        let task = self.tasks().into_iter().find(|t| t.id == id);
        let Some(task) = task else {
            return err("unknown_request", format!("没有后台任务 {id}"));
        };
        if task.status != "running" {
            return err("bad_request", format!("后台任务 {id} 已经结束（{}）", task.status));
        }
        // The panel shows a shell by its command and an agent or a Workflow by its description.
        let needle = match task.kind {
            "shell" => task.command.clone(),
            _ => task.description.clone(),
        }
        .unwrap_or_default();
        let needle = recognize::collapse_ws(&needle);
        if needle.is_empty() {
            return err("unrecognized_screen", format!("后台任务 {id} 没有可在面板里比对的命令或描述"));
        }
        let fits = |text: &str| recognize::collapse_ws(text).contains(&needle);
        self.slash("tasks")?;
        let Some((panel, s)) = self.wait_screen(Duration::from_secs(3), |_, s| recognize::tasks_panel(&s.lines))
        else {
            return Err(self.unrecognized("stop_task", None));
        };
        match panel {
            recognize::TasksPanel::Details(text) => {
                if !fits(&text) || !text.contains("running") {
                    self.keys(b"\x1b")?;
                    return Err(self.unrecognized("stop_task", Some(&s)));
                }
            }
            recognize::TasksPanel::List(rows) => {
                match recognize::task_row(&rows, &needle) {
                    Ok(_) => {}
                    Err(why) => {
                        self.keys(b"\x1b")?;
                        self.emit(json!({"type": "unrecognized", "context": format!("stop_task：{why}"), "lines": s.lines}));
                        return err("unrecognized_screen", format!("stop_task：{why}"));
                    }
                };
                // Move one row at a time (group titles are skipped by the cursor) until the
                // cursor is on the target row.
                let mut moved = false;
                for _ in 0..=rows.len() {
                    let Some(recognize::TasksPanel::List(now)) =
                        self.screen().ok().and_then(|s| recognize::tasks_panel(&s.lines))
                    else {
                        break;
                    };
                    // Found afresh each time: a running row's timer may change between frames.
                    let (Some(cur), Ok(want)) = (
                        now.iter().position(|(_, sel)| *sel),
                        recognize::task_row(&now, &needle),
                    ) else {
                        break;
                    };
                    if cur == want {
                        moved = true;
                        break;
                    }
                    let key: &[u8] = if want > cur { b"\x1b[B" } else { b"\x1b[A" };
                    let before = self.conn.generation();
                    self.keys(key)?;
                    self.conn.wait_change(before, Duration::from_millis(500));
                }
                if !moved {
                    self.keys(b"\x1b")?;
                    return Err(self.unrecognized("stop_task", None));
                }
            }
        }
        self.keys(b"x")?;
        // The panel shows it at once; Claude Code tells the transcript only on the next turn.
        let ended = self.wait_screen(Duration::from_secs(5), |_, s| match recognize::tasks_panel(&s.lines) {
            Some(recognize::TasksPanel::List(rows)) => recognize::task_row(&rows, &needle).is_err().then_some(()),
            Some(recognize::TasksPanel::Details(t)) => (!t.contains("running")).then_some(()),
            // The panel closed: back at the input box (not replaced by some other dialog).
            None => recognize::input_box(&s.lines).map(|_| ()),
        });
        if self.screen().ok().and_then(|s| recognize::tasks_panel(&s.lines)).is_some() {
            self.keys(b"\x1b")?;
        }
        if ended.is_none() {
            return err("timeout", format!("5 秒内面板上的后台任务 {id} 仍在运行"));
        }
        let tid = id.to_string();
        if let Some(t) = self.tailer.as_mut() {
            t.mark_stopped(&tid);
        }
        // Claude Code's notice (`killed`) reaches the plugin at once when the main line is idle,
        // else once the running tool ends; until then the task is `stopped`.
        let status_of = |e: &Engine| e.tasks().into_iter().find(|t| t.id == tid).map(|t| t.status);
        if status_of(self).as_deref() == Some("running") {
            self.set_task_status(&tid, "stopped");
        }
        self.wait_until(Duration::from_secs(2), |e| {
            status_of(e).is_some_and(|s| s != "running" && s != "stopped")
        });
        Ok(json!({"status": status_of(self)}))
    }

    fn get_state(&self) -> Value {
        let mut v = self.state.to_json();
        let request = self.announced.as_ref().map(|id| {
            if id.starts_with("choice:")
                && let Some(ev) = &self.choice_event
            {
                return ev.clone();
            }
            match self.open_request(id) {
                Some(r) => json!({"request_id": r.id, "tool_name": r.tool_name, "input": r.input,
                    "agent_id": r.agent_id}),
                None => json!({"waiting_for": id.strip_prefix("dialog:").unwrap_or(id)}),
            }
        });
        v["request"] = request.unwrap_or(Value::Null);
        v["holder_id"] = json!(self.holder_id);
        v["session_id"] = json!(self.session_id);
        v["cwd"] = json!(self.pf.as_ref().and_then(|p| p.cwd.clone()));
        v["follow_up_queue"] = json!(self.follow_ups.len());
        v["follow_ups"] = json!(self.follow_ups);
        v["taken_over"] = json!(self.taken_over);
        v
    }

    fn get_messages(&self, since: Option<u64>) -> CmdResult {
        let Some(t) = &self.tailer else {
            return Ok(json!({"blocks": [], "cursor": 0}));
        };
        let (blocks, cursor) = transcript::main_blocks(t.main_path(), since.unwrap_or(0));
        Ok(json!({"blocks": blocks, "cursor": cursor}))
    }

    pub fn execute(&mut self, c: &Command) -> CmdResult {
        let reads = matches!(
            c,
            Command::GetState
                | Command::GetMessages { .. }
                | Command::GetScreen
                | Command::GetToolResult { .. }
                | Command::GetTasks
                | Command::CancelFollowUp { .. }
                | Command::Reclaim { .. }
        );
        if self.taken_over && !reads {
            return err("taken_over", "Session 已被人接手，只读");
        }
        match c {
            Command::Prompt { text, images } => self.send_prompt_with(text, images),
            Command::CancelFollowUp { index } => self.cancel_follow_up(*index),
            Command::Reclaim { force } => self.reclaim(*force),
            Command::SetMode { mode } => self.set_mode(*mode),
            Command::GetTasks => Ok(json!({"tasks": self.tasks()})),
            Command::StopTask { task_id } => self.stop_task(task_id),
            Command::AnswerChoice { request_id, option } => self.answer_choice(request_id, option),
            Command::Steer { text } => self.steer(text),
            Command::FollowUp { text } => {
                self.require(true, "")?;
                Self::clean_text(text)?;
                self.follow_ups.push_back(text.clone());
                Ok(json!({"queued": self.follow_ups.len()}))
            }
            Command::Abort => self.abort(),
            Command::AnswerPermission {
                request_id,
                decision,
                message,
            } => self.answer_permission(request_id, decision, message.as_deref()),
            Command::AnswerQuestion {
                request_id,
                answers,
            } => self.answer_question(request_id, answers),
            Command::Slash { command } => self.slash(command),
            Command::SetModel { model } => self.slash(&format!("model {model}")),
            Command::Compact { instructions } => match instructions {
                Some(i) => self.slash(&format!("compact {i}")),
                None => self.slash("compact"),
            },
            Command::Clear => self.slash("clear"),
            Command::SendKeys { keys } => self.send_keys(keys),
            Command::GetState => Ok(self.get_state()),
            Command::GetMessages { since } => self.get_messages(*since),
            Command::GetScreen => {
                let s = self.screen().map_err(|e| ("io", format!("{e:#}")))?;
                Ok(json!({"lines": s.lines, "cursor": [s.cursor.0, s.cursor.1]}))
            }
            Command::GetToolResult { tool_use_id } => {
                let found = self.tailer.as_ref().and_then(|t| t.tool_result(tool_use_id));
                match found {
                    Some((content, persisted)) => {
                        Ok(json!({"content": content, "persisted": persisted}))
                    }
                    None => err("unknown_request", format!("找不到 {tool_use_id} 的工具结果")),
                }
            }
        }
    }

    fn ready(&self) {
        let version = self.pf.as_ref().and_then(|p| p.version.clone());
        let mut v = self.state.to_json();
        v["type"] = json!("ready");
        v["protocol"] = json!(PROTOCOL);
        v["holder_id"] = json!(self.holder_id);
        v["session_id"] = json!(self.session_id);
        v["cwd"] = json!(self.pf.as_ref().and_then(|p| p.cwd.clone()));
        v["claude_version"] = json!(version);
        v["size"] = json!([self.size.0, self.size.1]);
        v["taken_over"] = json!(self.taken_over);
        let mut warnings = Vec::new();
        if let Some(ver) = &version
            && !TESTED_CLAUDE_VERSIONS.contains(&ver.as_str())
        {
            warnings.push(format!("Claude Code {ver} 不是实测过的版本，认不出的界面会交给分析器或报 unrecognized"));
        }
        if !self.holder_reports_owner {
            warnings.push("Holder 不报告尺寸所有者，只能靠 PTY 尺寸变化判断接手".to_string());
        }
        v["warnings"] = json!(warnings);
        self.emit(v);
    }

    /// Runs until stdin closes.
    pub fn run(mut self, input: Receiver<String>) -> Result<()> {
        self.settle();
        if let Some(f) = self.fatal.take() {
            self.conn.detach();
            bail!(f);
        }
        self.ready();
        // Workflow Runs already there are reported once after `ready`.
        if let Some(t) = self.tailer.as_mut() {
            t.forget_workflows();
        }
        loop {
            if let Some(f) = self.fatal.take() {
                self.conn.detach();
                bail!(f);
            }
            // Taken before looking at stdin: a line arriving now still wakes the wait below.
            let seen = self.wake.seen();
            match input.try_recv() {
                Ok(line) => self.handle_line(&line),
                Err(TryRecvError::Empty) => {
                    self.tick();
                    self.drain_follow_ups();
                    self.wake.wait(seen, IDLE_TICK);
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }
        self.conn.detach();
        Ok(())
    }

    /// First observation without events (they would describe the past); a turn already running
    /// stays open so its end is still reported.
    fn settle(&mut self) {
        self.quiet = true;
        self.last_locate = None;
        self.tick();
        self.quiet = false;
        // A turn already running is adopted: its Stop (whatever its prompt id) ends it.
        self.turn_active = matches!(self.state, State::Busy | State::Waiting { .. });
        if self.turn_active {
            self.turn = TurnStats::start(None, "prompt");
        }
        self.announced = None;
        self.startup_checked = None;
        self.startup_last = None;
    }

    fn handle_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        match rpc::parse_line(line) {
            Ok(Request { id, command }) => {
                let name = rpc::command_name(&command);
                let v = match self.execute(&command) {
                    Ok(data) => rpc::response_ok(id.as_deref(), name, data),
                    Err((code, msg)) => rpc::response_err(id.as_deref(), name, code, &msg),
                };
                self.emit(v);
            }
            Err(RpcError {
                id,
                command,
                code,
                message,
            }) => {
                self.emit(rpc::response_err(
                    id.as_deref(),
                    command.as_deref().unwrap_or(""),
                    code,
                    &message,
                ));
            }
        }
    }
}

fn stdin_lines(wake: Arc<Wake>) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
            wake.bump();
        }
        // stdin closed: wake the loop so it sees the disconnect.
        wake.bump();
    });
    rx
}

fn run_engine(e: Engine) -> Result<()> {
    let lines = stdin_lines(e.wake.clone());
    e.run(lines)
}

/// Options of `new` / `attach`.
pub struct Opts {
    /// PTY size ccrpc sets.
    pub size: (u16, u16),
    /// Connect even when someone else holds the PTY size.
    pub force: bool,
}

/// `COLSxROWS`.
fn parse_size(s: &str) -> Result<(u16, u16)> {
    let (c, r) = s.split_once('x').context("--size 的格式是 COLSxROWS，如 160x48")?;
    let (c, r): (u16, u16) = (
        c.parse().context("--size 的列数不对")?,
        r.parse().context("--size 的行数不对")?,
    );
    if c < 40 || r < 10 || c > 1000 || r > 500 {
        bail!("--size 须在 40x10 到 1000x500 之间");
    }
    Ok((c, r))
}

/// `attach <Holder ID | sessionId>`.
pub fn resolve_target(roots: &Roots, target: &str) -> Result<String> {
    if locate::valid_holder_id(target) && roots.holder_socket(target).exists() {
        return Ok(target.to_string());
    }
    // A sessionId: find its process via the Process Files, then the Holder above it.
    let rd =
        std::fs::read_dir(roots.sessions_dir()).context("读不到 Claude Code 的 sessions 目录")?;
    let holders = locate::list_holders(roots);
    let table = locate::process_table()?;
    for e in rd.flatten() {
        let p: PathBuf = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let Ok(pf) = serde_json::from_slice::<ProcessFile>(&bytes) else {
            continue;
        };
        if pf.session_id.as_deref() != Some(target) {
            continue;
        }
        let Some(pid) = pf.pid else { continue };
        // A pid reused by another process is not this Session (procStart check).
        if !procfile::is_alive(&pf) {
            continue;
        }
        if let Some((id, _)) = locate::holder_of_pid(&table, &holders, pid) {
            return Ok(id.clone());
        }
        bail!("Session {target} 不在 clay 的 Holder 里运行，无法控制");
    }
    bail!("找不到 {target}：既不是运行中的 Holder，也不是运行中的 Session")
}

pub fn cmd_attach(roots: &Roots, args: &[String]) -> Result<()> {
    let mut opts = Opts {
        size: DEFAULT_SIZE,
        force: false,
    };
    let mut target = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--force" => opts.force = true,
            "--size" => opts.size = parse_size(it.next().context("--size 需要 COLSxROWS")?)?,
            t if target.is_none() && !t.starts_with('-') => target = Some(t.to_string()),
            _ => bail!("用法：ccrpc attach [--force] [--size COLSxROWS] <Holder ID | sessionId>"),
        }
    }
    let Some(target) = target else {
        bail!("用法：ccrpc attach [--force] [--size COLSxROWS] <Holder ID | sessionId>");
    };
    let target = &target;
    let holder = resolve_target(roots, target)?;
    let meta = locate::read_holder(roots, &holder)?;
    let table = locate::process_table()?;
    let Some(claude) = locate::find_claude_under(&table, meta.child_pid) else {
        bail!("Holder {holder} 里没有运行中的 Claude Code");
    };
    let checkin = hook::read_checkin(&hook::state_dir(), claude.pid);
    let pf = procfile::read(roots, claude.pid);
    if let Some(problem) =
        hook::checkin_problem(checkin.as_ref(), pf.as_ref().and_then(|p| p.session_id.as_deref()))
    {
        bail!(problem);
    }
    if hook::socket_in_use(&hook::state_dir(), claude.pid) {
        bail!("已有 ccrpc 连着这个 Session");
    }
    run_engine(Engine::open(roots.clone(), &holder, Out::stdout(), &opts)?)
}

pub fn cmd_new(roots: &Roots, args: &[String]) -> Result<()> {
    let (opts, claude_args) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let mut cwd = None;
    // A Pane ccrpc just started is not in anyone's use, whoever shows it.
    let mut eng = Opts {
        size: DEFAULT_SIZE,
        force: true,
    };
    let mut it = opts.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cwd" => cwd = Some(it.next().context("--cwd 需要目录")?.clone()),
            "--size" => eng.size = parse_size(it.next().context("--size 需要 COLSxROWS")?)?,
            other => bail!("new：不认识的参数 {other}；claude 的参数放在 -- 之后"),
        }
    }
    // The plugin must be there for the Session to be controllable (ADR-0003).
    if !claude_args.iter().any(|a| a == "--plugin-dir") && !crate::plugin::enabled(roots) {
        bail!("ccrpc 插件未启用：先运行 ccrpc plugin install");
    }
    let mut cmd = std::process::Command::new("toh");
    cmd.arg("new");
    if let Some(d) = &cwd {
        cmd.args(["--cwd", d]);
    }
    cmd.arg("--");
    // `toh new` passes only a few variables to the Pane: carry a non-default config dir explicitly
    // so claude writes its Process File where ccrpc reads.
    // So does a non-default ccrpc state directory, for the plugin's hooks.
    let mut env = Vec::new();
    if roots.claude_config != Roots::default_claude_config() {
        env.push(format!("CLAUDE_CONFIG_DIR={}", roots.claude_config.display()));
    }
    if let Some(d) = std::env::var_os("CCRPC_STATE_DIR").filter(|v| !v.is_empty()) {
        env.push(format!("CCRPC_STATE_DIR={}", PathBuf::from(d).display()));
    }
    if !env.is_empty() {
        cmd.arg("env").args(env);
    }
    cmd.arg("claude").args(claude_args);
    cmd.env("TOH_STATE_DIR", &roots.toh_state);
    for k in [
        "TOH_CONFIG",
        "KITTY_WINDOW_ID",
        "KITTY_LISTEN_ON",
        "WEZTERM_PANE",
        "WEZTERM_UNIX_SOCKET",
        "KAKU_UNIX_SOCKET",
        "ITERM_SESSION_ID",
        "TOH_HOLDER_ID",
        "CLAUDECODE",
        "CCRPC_ANALYZER_API_KEY",
    ] {
        cmd.env_remove(k);
    }
    let out = cmd
        .output()
        .context("运行 toh new 失败（toh 在 PATH 里吗？）")?;
    if !out.status.success() {
        bail!(
            "toh new 失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let holder = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !locate::valid_holder_id(&holder) {
        bail!("toh new 没有返回 Holder ID");
    }
    run_engine(Engine::open(roots.clone(), &holder, Out::stdout(), &eng)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_compare_by_sequence_not_length() {
        let mut m = Marks::default();
        for i in 0..100 {
            m.prompt(format!("u{i}"), format!("p{i}"));
        }
        let before = m.seq;
        assert!(
            m.prompt_after(before, "p99").is_none(),
            "old prompts do not count"
        );
        m.prompt("new".into(), "hello ".into());
        assert_eq!(m.prompt_after(before, "hello"), Some("new"));
        for i in 0..100 {
            m.enqueue(format!("s{i}"));
        }
        let b2 = m.seq;
        m.enqueue("steer".into());
        assert!(m.enqueued_after(b2, "steer"));
        assert!(!m.enqueued_after(b2, "s5"));
    }

    #[test]
    fn needles_for_dialog_matching() {
        assert_eq!(
            dialog_needle("Bash", &json!({"command": "touch  created.txt"})).as_deref(),
            Some("touchcreated.txt")
        );
        assert_eq!(
            dialog_needle("Edit", &json!({"file_path": "/a/b/c.rs"})).as_deref(),
            Some("c.rs")
        );
        assert_eq!(dialog_needle("mcp__x__y", &json!({"a": 1})), None);
    }

    #[test]
    fn question_answers_follow_the_dialog() {
        let qs = vec![
            json!({"question": "Which linters?", "multiSelect": true,
                "options": [{"label": "eslint"}, {"label": "ruff"}, {"label": "clippy"}]}),
            json!({"question": "Indent style?", "multiSelect": false,
                "options": [{"label": "Tabs"}, {"label": "Spaces"}]}),
        ];
        let a = question_answers(
            &qs,
            &[
                Answer::Options(vec!["eslint".into(), "clippy".into()]),
                Answer::Other("two\nspaces".into()),
            ],
        )
        .unwrap();
        assert_eq!(a["Which linters?"], "eslint, clippy");
        assert_eq!(a["Indent style?"], "two spaces");
        // Other on a multi-select is free text too.
        assert!(question_answers(&qs, &[Answer::Other("all".into()), Answer::Options(vec!["Tabs".into()])]).is_ok());
        assert!(question_answers(&qs, &[Answer::Options(vec!["nope".into()]), Answer::Options(vec!["Tabs".into()])]).is_err());
        assert!(question_answers(&qs, &[Answer::Options(vec![]), Answer::Options(vec!["Tabs".into()])]).is_err());
        assert!(
            question_answers(&qs, &[Answer::Options(vec!["ruff".into()]), Answer::Options(vec!["Tabs".into(), "Spaces".into()])])
                .is_err(),
            "single choice"
        );
        assert!(question_answers(&qs[..1], &[]).is_err(), "one answer per question");
    }
}
