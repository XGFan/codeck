//! The Controlled Session: one engine thread owns the Holder connection, the Process File view
//! and the transcript tail; it executes commands in order and emits events. Commands that wait
//! keep ticking, so events keep flowing while they wait (follow-ups are only sent from the top
//! level loop, never from inside a waiting command).

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::analyze::{self, Analyzer};
use crate::layout::{self, Layout, LayoutKind, Library, Meaning};
use crate::locate::{self, Roots};
use crate::procfile::{self, ProcessFile};
use crate::pty::{self, HolderConn, Owner};
use crate::recognize::{self, AskScreen, PermissionChoice, PermissionDialog};
use crate::rpc::{self, Answer, Command, Decision, Out, Request, RpcError};
use crate::screen::Screen;
use crate::transcript::{self, BlockKind, PendingTool, Tailer, TxEvent};
use crate::wake::{FsWatch, Wake};

pub const PROTOCOL: u32 = 1;
/// Longest wait between two looks while a command waits.
const TICK: Duration = Duration::from_millis(50);
/// Longest wait between two looks when idle (file, Holder and stdin events wake earlier).
const IDLE_TICK: Duration = Duration::from_millis(250);
/// After entering idle, how long to wait for the `turn_duration` marker before `turn_end`.
const TURN_END_GRACE: Duration = Duration::from_secs(2);
/// `input needed` without a pending AskUserQuestion becomes `dialog` after this long.
const CLASSIFY_GRACE: Duration = Duration::from_millis(1500);
/// A permission prompt whose dialog matches no pending tool is reported as `dialog` after this long.
const MATCH_GRACE: Duration = Duration::from_secs(3);
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
}

impl TurnStats {
    fn start() -> TurnStats {
        TurnStats {
            started: Some(Instant::now()),
            cost: Some(0.0),
            ..Default::default()
        }
    }

    fn to_json(&self) -> Value {
        let measured = self.started.map(|t| t.elapsed().as_millis() as u64);
        json!({
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
fn dialog_needle(p: &PendingTool) -> Option<String> {
    let field = match p.name.as_str() {
        "Bash" => "command",
        "Edit" | "Write" | "MultiEdit" | "Read" | "NotebookEdit" => "file_path",
        "WebFetch" => "url",
        "Glob" | "Grep" => "pattern",
        _ => return None,
    };
    let v = p.input.get(field)?.as_str()?;
    let v = if field == "file_path" {
        v.rsplit('/').next().unwrap_or(v)
    } else {
        v
    };
    let n: String = v.chars().filter(|c| !c.is_whitespace()).take(40).collect();
    (!n.is_empty()).then_some(n)
}

pub struct Engine {
    roots: Roots,
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
    last_switch_try: Option<Instant>,
    pf: Option<ProcessFile>,
    session_id: Option<String>,
    tailer: Option<Tailer>,
    state: State,
    /// Raw `(status, waitingFor)` last read, with when it was first seen.
    raw_since: Option<((String, Option<String>), Instant)>,
    turn_end_due: Option<Instant>,
    /// A turn is running (a prompt was seen or the state was busy) and has not been closed.
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
    /// Lookups of the current sessionId's transcript so far (a file found on the first one is history).
    lookups: u32,
    /// Requests that are over (answered here or elsewhere): never matched to a dialog again.
    resolved: HashSet<String>,
    /// Last time the transcript produced anything.
    last_activity: Instant,
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
            last_switch_try: None,
            pf: None,
            session_id: None,
            tailer: None,
            state: State::Starting,
            raw_since: None,
            turn_end_due: None,
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
            lookups: 0,
            resolved: HashSet::new(),
            last_activity: Instant::now(),
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
        }
    }

    /// Follows the Process File's sessionId to its transcript. A transcript found here is read
    /// once as history: its blocks are not emitted, but prompts / enqueues still confirm commands
    /// and pending tools are known.
    fn switch_session(&mut self, pf: &ProcessFile) {
        let Some(sid) = pf.session_id.clone() else {
            return;
        };
        if self.session_id.as_deref() == Some(&sid) && self.tailer.is_some() {
            return;
        }
        if self.session_id.as_deref() != Some(&sid) {
            let prev = self.session_id.replace(sid.clone());
            self.tailer = None;
            self.last_switch_try = None;
            self.lookups = 0;
            if let Some(prev) = prev {
                self.emit(json!({"type": "session_changed", "session_id": sid, "previous": prev}));
            }
        }
        // Looking for the file lists the projects directory: at most twice a second.
        if self
            .last_switch_try
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        self.last_switch_try = Some(Instant::now());
        self.lookups += 1;
        let cwd = pf.cwd.clone().unwrap_or_default();
        let Some(path) = locate::transcript_path(&self.roots, &cwd, &sid) else {
            return;
        };
        let mut t = Tailer::new(&path);
        // A file that already existed when this sessionId was first seen is history; one created
        // afterwards (first prompt of a new Session, after /clear) is live and read by `tick`.
        if self.lookups == 1 {
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
                self.switch_session(&pf);
                self.pf = Some(pf);
            }
        }
        self.update_watches();
        let evs = self.tailer.as_mut().map(|t| t.poll()).unwrap_or_default();
        if !evs.is_empty() {
            self.last_activity = Instant::now();
        }
        let mut saw_turn_end = false;
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
                    let mut v = serde_json::to_value(&b).unwrap_or(Value::Null);
                    if let Value::Object(m) = &mut v {
                        m.insert("type".into(), json!("block"));
                    }
                    self.emit(v);
                }
                TxEvent::TurnEnd { duration_ms } => {
                    saw_turn_end = true;
                    if duration_ms.is_some() {
                        self.turn.duration_ms = duration_ms;
                    }
                }
                TxEvent::Usage {
                    model,
                    usage,
                    logged_cost,
                } => {
                    let t = &mut self.turn;
                    t.input += usage.input;
                    t.output += usage.output;
                    t.cache_read += usage.cache_read;
                    t.cache_creation += usage.cache_write_5m + usage.cache_write_1h;
                    let c = self.prices.cost(model.as_deref(), &usage, logged_cost);
                    t.cost = t.cost.zip(c).map(|(a, b)| a + b);
                }
                TxEvent::ApiError { kind, message } => {
                    self.turn.error = Some(json!({"kind": kind, "message": message}));
                }
                TxEvent::UserPrompt { uuid, text } => {
                    // A new prompt opens a new turn; close the previous one first.
                    if self.turn_end_due.is_some() {
                        self.emit_turn_end();
                    }
                    if !self.turn_active {
                        self.turn = TurnStats::start();
                    }
                    self.turn_active = true;
                    self.last_text = None;
                    self.marks.prompt(uuid, text);
                }
                TxEvent::Enqueued { content } => self.marks.enqueue(content),
                TxEvent::Branch { at_uuid } => {
                    self.emit(json!({"type": "branch", "at_uuid": at_uuid}));
                }
                TxEvent::Workflow(v) => self.emit(v),
            }
        }
        self.update_state();
        // The marker counts once the state agrees the turn is over (markers written while still
        // busy belong to a background Subagent hand-off, not to the end of the turn).
        if saw_turn_end
            && self.state == State::Idle
            && (self.turn_end_due.is_some() || self.turn_active)
        {
            self.emit_turn_end();
        }
        if self.turn_end_due.is_some_and(|t| Instant::now() >= t) {
            self.emit_turn_end();
        }
        // A turn that opened (a prompt was seen) but never went busy nor wrote its marker — an
        // immediate error, an abort within the busy lag — ends after a quiet spell.
        if self.turn_active
            && self.state == State::Idle
            && self.turn_end_due.is_none()
            && self.last_activity.elapsed() >= Duration::from_secs(5)
        {
            self.emit_turn_end();
        }
        self.refresh_announcement();
        self.announce_requests();
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

    fn emit_turn_end(&mut self) {
        self.turn_end_due = None;
        self.turn_active = false;
        let mut v = self.turn.to_json();
        v["type"] = json!("turn_end");
        v["last_text"] = json!(self.last_text);
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
        // A new turn started before the previous one's marker arrived: close the previous turn first.
        if self.turn_end_due.is_some() && matches!(s, State::Busy | State::Waiting { .. }) {
            self.emit_turn_end();
        }
        // A turn is something that ran; a dialog that opens and closes from idle is not.
        if s == State::Busy {
            if !self.turn_active {
                self.turn = TurnStats::start();
            }
            self.turn_active = true;
        }
        if s == State::Idle && self.turn_active {
            self.turn_end_due = Some(Instant::now() + TURN_END_GRACE);
        }
        if matches!(
            self.state,
            State::Waiting {
                kind: WaitKind::Permission | WaitKind::Question,
                ..
            }
        ) && let Some(id) = self.announced.take()
        {
            self.resolved.insert(id);
        }
        self.announced = None;
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
                        let has_ask = self
                            .tailer
                            .as_ref()
                            .and_then(|t| t.pending_named("AskUserQuestion"))
                            .is_some();
                        if has_ask {
                            WaitKind::Question
                        } else if since.elapsed() < CLASSIFY_GRACE {
                            return; // the tool_use may not be written yet
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
                .is_some_and(|tid| self.tailer.as_ref().is_some_and(|t| t.pending_by_id(tid).is_none()));
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
        if self
            .tailer
            .as_ref()
            .and_then(|t| t.pending_by_id(id))
            .is_none()
        {
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

    fn permission_target(&self, s: &Screen) -> Option<PendingTool> {
        let (_, body) = self.dialog_of(&s.lines)?;
        let t = self.tailer.as_ref()?;
        let cands: Vec<&PendingTool> = t
            .pending()
            .into_iter()
            .filter(|p| !NO_PERMISSION_TOOLS.contains(&p.name.as_str()))
            .filter(|p| !self.resolved.contains(&p.id))
            .collect();
        let matched: Vec<&&PendingTool> = cands
            .iter()
            .filter(|p| dialog_needle(p).is_some_and(|n| body.contains(&n)))
            .collect();
        // Exactly one tool may match; two that both fit the dialog are ambiguous.
        match matched.len() {
            1 => return Some((*matched[0]).clone()),
            0 => {}
            _ => return None,
        }
        let blind: Vec<&&PendingTool> = cands
            .iter()
            .filter(|p| dialog_needle(p).is_none())
            .collect();
        (blind.len() == 1 && cands.len() == 1).then(|| (*blind[0]).clone())
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
        if self.announced.is_some() {
            return;
        }
        match kind {
            WaitKind::Permission => {
                let screen = self.screen().ok();
                if let Some(p) = screen.as_ref().and_then(|s| self.permission_target(s)) {
                    // Which layout recognized it, when not the built-in recognizer.
                    let layout = screen.as_ref().and_then(|s| {
                        recognize::permission(&s.lines)
                            .is_none()
                            .then(|| {
                                self.library
                                    .find_kind(&s.lines, LayoutKind::Permission)
                                    .map(|(l, _)| (l.id.clone(), l.source.clone()))
                            })
                            .flatten()
                    });
                    self.announced = Some(p.id.clone());
                    self.emit(json!({"type": "permission_request", "request_id": p.id, "tool_name": p.name,
                        "input": p.input, "agent_id": p.agent_id,
                        "layout": layout.as_ref().map(|l| &l.0), "learned": layout.is_some_and(|l| l.1 == "learned")}));
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
                // The tool_use may still be on its way: give the transcript time before asking
                // the analyzer about a dialog that may simply not be matched yet.
                if self.waiting_age() >= MATCH_GRACE {
                    if self.analyzing(&s, &waiting_for) {
                        return;
                    }
                    // Cannot tie the dialog to a tool: hand the screen to the caller.
                    self.announced = Some(format!("dialog:{waiting_for}"));
                    self.emit(
                        json!({"type": "dialog", "waiting_for": waiting_for, "lines": s.lines}),
                    );
                }
            }
            WaitKind::Question => {
                let Some(p) = self
                    .tailer
                    .as_ref()
                    .and_then(|t| t.pending_named("AskUserQuestion"))
                    .cloned()
                else {
                    return;
                };
                self.announced = Some(p.id.clone());
                self.emit(json!({"type": "question", "request_id": p.id,
                    "questions": p.input.get("questions").cloned().unwrap_or(Value::Null), "agent_id": p.agent_id}));
            }
            WaitKind::Dialog => {
                // The Process File says waiting before the dialog is drawn: let it render first
                // (a permission prompt is only taken once recognized, a question comes from the
                // transcript).
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
            let tools_ok = self.tailer.as_ref().is_some_and(|t| {
                let tools: Vec<_> = t
                    .pending()
                    .into_iter()
                    .filter(|p| !self.resolved.contains(&p.id))
                    .filter(|p| !NO_PERMISSION_TOOLS.contains(&p.name.as_str()))
                    .collect();
                !tools.is_empty() && tools.iter().all(|p| CHOICE_TOOLS.contains(&p.name.as_str()))
            });
            if !tools_ok {
                return false;
            }
        }
        let (id, title) = (l.id.clone(), l.title.clone());
        // A choice raised by a tool (ExitPlanMode…): the single pending tool, if any.
        let tool = self.tailer.as_ref().and_then(|t| {
            let p: Vec<_> = t
                .pending()
                .into_iter()
                .filter(|p| !self.resolved.contains(&p.id))
                .collect();
            (p.len() == 1).then(|| p[0].id.clone())
        });
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
        let pending: Vec<(String, Value)> = self
            .tailer
            .as_ref()
            .map(|t| {
                t.pending()
                    .into_iter()
                    .filter(|p| !self.resolved.contains(&p.id))
                    .map(|p| (p.name.clone(), p.input.clone()))
                    .collect()
            })
            .unwrap_or_default();
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
        if self.state != State::Idle || self.turn_active || self.turn_end_due.is_some() {
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
        // Idle and no turn open (a prompt just submitted shows busy only ~1 s later). A turn that is
        // only waiting for its end marker closes within a few seconds: wait for that.
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
            match std::fs::canonicalize(p) {
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
        Ok(json!({"uuid": found(self)}))
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
                || e.tailer
                    .as_ref()
                    .is_some_and(|t| t.pending_by_id(&rid).is_none())
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
        let rid = request_id.to_string();
        // The dialog on screen must be the one for this request.
        let found = self.wait_screen(Duration::from_secs(3), |e, s| {
            let (d, _) = e.dialog_of(&s.lines)?;
            (e.permission_target(s)?.id == rid).then_some(d)
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
        // Last check right before the key: still the same request, on a Snapshot taken now.
        let now = self.fresh_screen()?;
        if !self.request_is(&WaitKind::Permission, request_id)
            || self.permission_target(&now).is_none_or(|p| p.id != rid)
        {
            return err(
                "unknown_request",
                format!("作答前请求已经变了，不再是 {request_id}"),
            );
        }
        self.keys(n.to_string().as_bytes())?;
        let mut res = self.wait_leave_request(request_id)?;
        if let Some(msg) = message {
            if !self.wait_until(Duration::from_secs(10), |e| e.state == State::Idle) {
                return err("timeout", "拒绝后 10 秒内没有回到 idle，说明没有发出");
            }
            let sent = self.send_prompt(msg)?;
            res["message_uuid"] = sent["uuid"].clone();
        }
        Ok(res)
    }

    fn answer_question(&mut self, request_id: &str, answers: &[Answer]) -> CmdResult {
        self.check_request(WaitKind::Question, request_id)?;
        let questions = self
            .tailer
            .as_ref()
            .and_then(|t| t.pending_by_id(request_id))
            .and_then(|p| p.input.get("questions").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        if questions.len() != answers.len() {
            return err(
                "bad_request",
                format!("有 {} 道题，给了 {} 个答案", questions.len(), answers.len()),
            );
        }
        // Validate everything before pressing the first key.
        let mut plan = Vec::new();
        for (i, (q, a)) in questions.iter().zip(answers).enumerate() {
            let title = q
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let multi = q
                .get("multiSelect")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let labels: Vec<String> = q
                .get("options")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|o| o.get("label").and_then(Value::as_str).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            match a {
                Answer::Options(chosen) => {
                    if let Some(bad) = chosen.iter().find(|c| !labels.contains(c)) {
                        return err("bad_request", format!("第 {} 题没有选项 {bad}", i + 1));
                    }
                    if !multi && chosen.len() != 1 {
                        return err("bad_request", format!("第 {} 题是单选", i + 1));
                    }
                }
                Answer::Other(_) if multi => return err("unsupported", "多选题暂不支持 other"),
                Answer::Other(_) => {}
            }
            plan.push((title, a, multi));
        }
        let mut tabbed = false;
        for (title, a, multi) in plan {
            if !self.request_is(&WaitKind::Question, request_id) {
                return err(
                    "unknown_request",
                    format!("作答中请求已经变了，不再是 {request_id}"),
                );
            }
            tabbed |= self.answer_one(&title, a, multi)?;
        }
        // Several questions end on the Review tab.
        if tabbed {
            let review = self.wait_screen(Duration::from_secs(3), |_, s| {
                match recognize::ask(&s.lines) {
                    Some(AskScreen::Review { submit }) => Some(submit),
                    _ => None,
                }
            });
            let Some((submit, _)) = review else {
                return Err(self.unrecognized("answer_question", None));
            };
            self.keys(submit.to_string().as_bytes())?;
        }
        self.wait_leave_request(request_id)
    }

    /// Answers one question; returns whether the UI is tabbed (several questions).
    fn answer_one(
        &mut self,
        title: &str,
        a: &Answer,
        multi: bool,
    ) -> std::result::Result<bool, CmdErr> {
        let want = recognize::collapse_ws(title);
        let found = self.wait_screen(Duration::from_secs(3), |_, s| {
            match recognize::ask(&s.lines) {
                Some(AskScreen::Question(q)) if q.title == want => Some(q),
                _ => None,
            }
        });
        let Some((q, s)) = found else {
            return Err(self.unrecognized("answer_question", None));
        };
        match a {
            Answer::Other(text) => {
                let Some(n) = q.other else {
                    return Err(self.unrecognized("answer_question", Some(&s)));
                };
                self.keys(n.to_string().as_bytes())?;
                // Type only once the free-text field is active (digits would pick options).
                let editing =
                    self.wait_screen(Duration::from_secs(2), |_, s| {
                        match recognize::ask(&s.lines) {
                            Some(AskScreen::Question(q)) if q.title == want && q.editing => {
                                Some(())
                            }
                            _ => None,
                        }
                    });
                if editing.is_none() {
                    return Err(self.unrecognized("answer_question", None));
                }
                let clean = pty::sanitize(text).replace(['\n', '\t'], " ");
                self.keys(clean.as_bytes())?;
                self.keys(b"\r")?;
            }
            Answer::Options(chosen) if !multi => {
                let Some(o) = q.options.iter().find(|o| o.label == chosen[0]) else {
                    return Err(self.unrecognized("answer_question", Some(&s)));
                };
                self.keys(o.number.to_string().as_bytes())?;
            }
            Answer::Options(chosen) => {
                if q.options.len() != q.options.iter().filter(|o| o.checked.is_some()).count() {
                    return Err(self.unrecognized("answer_question", Some(&s)));
                }
                for o in &q.options {
                    if o.checked != Some(chosen.contains(&o.label)) {
                        self.keys(o.number.to_string().as_bytes())?;
                    }
                }
                if q.tabbed {
                    self.keys(b"\x1b[C")?;
                } else {
                    // Single multi-select question: move the cursor onto the Submit row and press Enter.
                    let (Some(sub), Some(cur)) = (q.submit_row, q.cursor_row) else {
                        return Err(self.unrecognized("answer_question", Some(&s)));
                    };
                    let rows_between = q
                        .options
                        .iter()
                        .filter(|o| o.row > cur && o.row < sub)
                        .count()
                        + usize::from(q.other.is_some());
                    for _ in 0..rows_between {
                        self.keys(b"\x1b[B")?;
                    }
                    let on_submit = self
                        .wait_screen(Duration::from_secs(2), |_, s| {
                            s.lines
                                .get(sub)
                                .filter(|l| l.trim_start().starts_with("❯ Submit"))
                                .map(|_| ())
                        })
                        .is_some();
                    if !on_submit {
                        return Err(self.unrecognized("answer_question", None));
                    }
                    self.keys(b"\r")?;
                }
            }
        }
        Ok(q.tabbed)
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
        let task = self
            .tailer
            .as_ref()
            .and_then(|t| t.tasks().iter().find(|t| t.id == id).cloned());
        let Some(task) = task else {
            return err("unknown_request", format!("没有后台任务 {id}"));
        };
        if task.status != "running" {
            return err("bad_request", format!("后台任务 {id} 已经结束（{}）", task.status));
        }
        // The panel shows a shell by its command and an agent by its description.
        let needles: Vec<String> = [task.command.clone(), task.description.clone()]
            .into_iter()
            .flatten()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(30).collect::<String>())
            .filter(|t| !t.trim().is_empty())
            .collect();
        if needles.is_empty() {
            return err("unrecognized_screen", format!("后台任务 {id} 没有可在面板里比对的描述"));
        }
        let fits = |text: &str| {
            let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
            needles.iter().any(|n| t.contains(n.trim()))
        };
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
                let target = match recognize::task_row(&rows, &needles) {
                    Ok(i) => rows[i].0.clone(),
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
                    let (Some(cur), Some(want)) = (
                        now.iter().position(|(_, sel)| *sel),
                        now.iter().position(|(t, _)| *t == target),
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
            Some(recognize::TasksPanel::List(rows)) => (!rows
                .iter()
                .any(|(t, _)| fits(t) && t.contains("running")))
            .then_some(()),
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
        let status = self
            .tailer
            .as_ref()
            .and_then(|t| t.tasks().iter().find(|t| t.id == tid))
            .map(|t| t.status.clone());
        Ok(json!({"status": status}))
    }

    fn get_state(&self) -> Value {
        let mut v = self.state.to_json();
        let request = self.announced.as_ref().map(|id| {
            if id.starts_with("choice:")
                && let Some(ev) = &self.choice_event
            {
                return ev.clone();
            }
            match self.tailer.as_ref().and_then(|t| t.pending_by_id(id)) {
                Some(p) => json!({"request_id": p.id, "tool_name": p.name, "input": p.input}),
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
            Command::GetTasks => Ok(json!({
                "tasks": self.tailer.as_ref().map(|t| t.tasks().to_vec()).unwrap_or_default()
            })),
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
        self.ready();
        // Workflow Runs already there are reported once after `ready`.
        if let Some(t) = self.tailer.as_mut() {
            t.forget_workflows();
        }
        loop {
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
        self.turn_end_due = None;
        self.turn_active = matches!(self.state, State::Busy | State::Waiting { .. });
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
    if locate::find_claude_under(&table, meta.child_pid).is_none() {
        bail!("Holder {holder} 里没有运行中的 Claude Code");
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
    let mut cmd = std::process::Command::new("toh");
    cmd.arg("new");
    if let Some(d) = &cwd {
        cmd.args(["--cwd", d]);
    }
    cmd.arg("--");
    // `toh new` passes only a few variables to the Pane: carry a non-default config dir explicitly
    // so claude writes its Process File where ccrpc reads.
    if roots.claude_config != Roots::default_claude_config() {
        cmd.arg("env").arg(format!(
            "CLAUDE_CONFIG_DIR={}",
            roots.claude_config.display()
        ));
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

    fn tool(name: &str, input: Value) -> PendingTool {
        PendingTool {
            id: "t".into(),
            name: name.into(),
            input,
            agent_id: None,
            seq: 1,
        }
    }

    #[test]
    fn needles_for_dialog_matching() {
        assert_eq!(
            dialog_needle(&tool("Bash", json!({"command": "touch  created.txt"}))).as_deref(),
            Some("touchcreated.txt")
        );
        assert_eq!(
            dialog_needle(&tool("Edit", json!({"file_path": "/a/b/c.rs"}))).as_deref(),
            Some("c.rs")
        );
        assert_eq!(dialog_needle(&tool("mcp__x__y", json!({"a": 1}))), None);
    }
}
