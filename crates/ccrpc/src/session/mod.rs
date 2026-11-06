//! The Controlled Session: one engine thread owns the Holder connection, the Process File view
//! and the transcript tail; it executes commands in order and emits events. Commands that wait
//! keep ticking, so events keep flowing while they wait, and stdin is still read: commands that
//! neither write to the Pane nor wait are answered at once, `abort` runs once the waiting command
//! has pressed its last key, the rest run after it (follow-ups are only sent from the top level
//! loop, never from inside a waiting command).

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::analysis_records::{self, Outcome, Record};
use crate::analyze::{self, Analyzer};
use crate::history::{self, HistoryWorker};
use crate::hook::{self, HookMsg, HookServer};
use crate::layout::{self, Layout, LayoutKind, Library, Meaning, Select};
use crate::locate::{self, Roots};
use crate::procfile::{self, ProcessFile};
use crate::pty::{self, HolderConn, key};
use crate::recognize::{self, PermissionDialog};
use crate::rpc::{self, Answer, Command, Decision, Out, Request, RpcError};
use crate::screen::Screen;
use crate::toh::{self, Pane};
use crate::transcript::{self, BgTask, BlockKind, Tailer, TaskKind, TxEvent};
use crate::wake::{FsWatch, Wake};

mod announce;
mod drive;
mod fork;
mod requests;
mod rewind;
mod startup;
mod tasks;
mod turn;

use self::announce::Announced;
pub use self::fork::ForkJob;
use self::requests::{HookPrompt, HookRequest, PreTool};
use self::rewind::RewindReq;
pub use self::startup::{cmd_attach, cmd_new, resolve_target};
use self::turn::{Marks, TurnTracker, Typed};

pub const PROTOCOL: u32 = 3;
/// Longest wait between two looks while a command waits.
const TICK: Duration = Duration::from_millis(50);
/// Longest wait between two looks when idle (file, Holder and stdin events wake earlier).
const IDLE_TICK: Duration = Duration::from_millis(250);
/// A turn whose Session went idle without a Stop (Escape, a deny that interrupts) ends as aborted
/// after this long (Stop arrives before idle).
const ABORT_GRACE: Duration = Duration::from_millis(1500);
/// The same for a turn moved to a background claude (agents page): a claimed spare reads idle
/// until it takes the interrupted reply up again.
const MOVED_GRACE: Duration = Duration::from_secs(5);
/// After Stop, how long to wait for the transcript to catch up (the turn's last message and its
/// `turn_duration` marker are written around the same moment) before `turn_end`.
const STOP_CATCH_UP: Duration = Duration::from_secs(2);
/// A follow-up goes out only after the Session has been idle this long: right after Stop, Claude
/// Code starts a turn of its own for what it queued (a background task's notification), and a
/// prompt typed then lands in its queue instead.
const FOLLOW_UP_SETTLE: Duration = Duration::from_secs(1);
/// `input needed` without an AskUserQuestion from the plugin becomes `dialog` after this long.
const CLASSIFY_GRACE: Duration = Duration::from_millis(500);
/// A permission prompt whose dialog matches no request is reported as `dialog` (or analyzed)
/// after this long.
const MATCH_GRACE: Duration = Duration::from_secs(1);
/// How long a main-line permission request waits for its dialog's options to be read off the
/// screen before it is announced without them.
const OPTIONS_GRACE: Duration = Duration::from_secs(1);
/// After an answer went back through a hook, the Process File and the screen still show the
/// closing dialog for a moment: nothing is announced or reclassified meanwhile.
const ANSWER_SETTLE: Duration = Duration::from_millis(1500);
/// claude's Process File is there but its plugin never checked in: not controllable.
const CHECKIN_GRACE: Duration = Duration::from_secs(5);
/// The folder trust dialog's layout id (built into the recognizer, not the library).
const TRUST_LAYOUT: &str = "trust_folder";
/// A dialog ignores keys for a moment after it appears (against accidental answers): keys that
/// answer one wait until it has been up this long.
const DIALOG_SETTLE: Duration = Duration::from_millis(500);
/// No Process File yet while claude runs: report the startup screen after this long.
const STARTUP_DIALOG_AFTER: Duration = Duration::from_secs(3);
/// Keys are written apart so Claude Code reads each one on its own: at least this long, and up to
/// [`KEY_SETTLE`] while the screen has not reacted yet.
const KEY_MIN_GAP: Duration = Duration::from_millis(50);
const KEY_SETTLE: Duration = Duration::from_millis(80);
/// PTY size ccrpc switches to when the screen cannot be read or written at the size people's
/// Clients set (a typical terminal, the size Claude Code is laid out for), and starts new Panes at
/// (ADR-0001).
pub const WORK_SIZE: (u16, u16) = (120, 40);
/// After switching to [`WORK_SIZE`], screens are not taken for anything until Claude Code has had
/// this long to redraw.
const GROW_SETTLE: Duration = Duration::from_millis(800);
/// Claude Code versions the built-in layouts were probed on.
pub const TESTED_CLAUDE_VERSIONS: &[&str] = &["2.1.316", "2.1.317"];
/// At most this many analyses an hour.
const MAX_ANALYSES_PER_HOUR: usize = 10;

/// A feedback draft as the caller gets it: `title`, `tool_use_id`, `input` (the `SendFeedback`
/// call, null when the transcript has none for the card).
fn public_draft(d: &Value) -> Value {
    json!({"title": d["title"], "tool_use_id": d["tool_use_id"], "input": d["input"]})
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

/// Failure of a command: `(code, message)`.
type CmdErr = (&'static str, String);
/// An analysis's layout (or why there is none) and its record.
type AnalysisDone = (std::result::Result<Layout, String>, Record);
type CmdResult = std::result::Result<Value, CmdErr>;

fn err<T>(code: &'static str, msg: impl Into<String>) -> std::result::Result<T, CmdErr> {
    Err((code, msg.into()))
}

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
    /// When the Session went idle (for a turn that never got its Stop).
    idle_at: Option<Instant>,
    /// When the Process File appeared without a check-in.
    no_checkin_since: Option<Instant>,
    /// An answer just went back through a hook: no announcing until then.
    answered_until: Option<Instant>,
    /// `get_messages` pages (its own thread).
    history: HistoryWorker,
    /// Main line's last model and context size; the permission mode the plugin last saw.
    model: Option<String>,
    /// The effort level: the TUI's `◉ xhigh · /effort` mark, what `/effort` said it set, else
    /// the last reply's; none for a model without effort levels.
    effort: Option<String>,
    context_tokens: Option<u64>,
    hook_mode: Option<String>,
    /// The mode line last seen (a dialog may cover it).
    screen_mode: Option<String>,
    /// `session_info` last sent.
    info_sent: Option<Value>,
    /// The feedback draft card last reported (`feedback_draft`), and since when it has been off
    /// the screen.
    feedback: Option<Value>,
    feedback_gone_since: Option<Instant>,
    /// Screen generation last looked at for the mode line and the spinner, and when.
    looked_gen: u64,
    looked_at: Option<Instant>,
    /// `progress` last sent: (text, when).
    progress_sent: Option<(String, Instant)>,
    /// The engine cannot go on (no plugin, another ccrpc): `run` returns this error.
    fatal: Option<String>,
    holder_id: String,
    shell_pid: u32,
    conn: HolderConn,
    out: Out,
    wake: Arc<Wake>,
    fs: FsWatch,
    /// PTY size last seen (people's Clients set it; ccrpc only when an operation failed).
    size: (u16, u16),
    /// When ccrpc last switched the PTY to [`WORK_SIZE`].
    grown_at: std::cell::Cell<Option<Instant>>,
    /// The Pane's claude: its exit ends the Session.
    claude_pid: Option<u32>,
    /// The background claude followed once the Pane's claude moved its conversation to the
    /// background (agents page): hooks, Process File and transcript are its from then on.
    followed: Option<u32>,
    /// The Pane's claude's `parkedJobId`, as last read.
    parked_job: Option<String>,
    /// The Pane's claude's lock, kept while its conversation is followed in the background: no
    /// other ccrpc attaches meanwhile (even while the background claude is stopped).
    pane_lock: Option<std::fs::File>,
    claude_seen_at: Option<Instant>,
    last_locate: Option<Instant>,
    /// Check-ins from this Holder already looked for in the process table.
    checkins_seen: HashSet<u32>,
    pf: Option<ProcessFile>,
    session_id: Option<String>,
    /// `ready` went out: a sessionId learned from now on is announced (`session_changed`).
    ready_sent: bool,
    tailer: Option<Tailer>,
    state: State,
    /// Raw `(status, waitingFor)` last read, with when it was first seen.
    raw_since: Option<((String, Option<String>), Instant)>,
    /// Turns: opened and ended by the plugin's hooks, with what each did.
    turns: TurnTracker,
    library: Library,
    analyzer: Option<Analyzer>,
    /// The analysis in flight: dialog signature and where its result arrives.
    analysis: Option<(String, Receiver<AnalysisDone>)>,
    /// Dialog signatures already analyzed (never twice).
    analyzed: HashSet<String>,
    analyses: VecDeque<Instant>,
    /// Numbers the choices announced (`choice:N`).
    choice_seq: u64,
    /// A rewind under way (the Rewind picker or its confirm page), and the ones numbered so far.
    rewind: Option<RewindReq>,
    rewind_seq: u64,
    /// The prompt a conversation restore went back before: the TUI's conversation ends before it
    /// until the transcript shows the next prompt.
    rewound: Option<String>,
    /// `new --fork`: the Fork still to be made (or done).
    fork: Option<ForkJob>,
    /// What went out for the current waiting.
    announced: Option<Announced>,
    startup_checked: Option<Instant>,
    startup_last: Option<Vec<String>>,
    /// Queued follow-ups (checked: image paths absolute).
    follow_ups: VecDeque<rpc::Input>,
    /// What the person typed (or a steer sent) waiting in Claude Code's own queue, oldest first:
    /// the transcript's queue minus task notifications and Teammate messages.
    queue: Vec<String>,
    marks: Marks,
    /// Requests that are over (answered here or elsewhere): never matched to a dialog again.
    resolved: HashSet<String>,
    procfile_warned: bool,
    exited: bool,
    /// Events are suppressed (initial settle).
    quiet: bool,
    /// stdin's lines (set by `run`).
    input: Option<Receiver<String>>,
    /// Lines read while a command ran and not served then: run after it, in order.
    deferred: VecDeque<String>,
    /// The running command has pressed its last key and only waits: an `abort` may run now.
    abort_point: bool,
}

/// Commands answered even while another one waits: they neither write to the Pane nor wait.
fn serves_in_wait(c: &Command) -> bool {
    matches!(
        c,
        Command::GetState
            | Command::GetScreen
            | Command::GetTasks
            | Command::GetToolResult { .. }
            | Command::GetImage { .. }
            | Command::FollowUp { .. }
            | Command::CancelFollowUp { .. }
    )
}

impl Engine {
    /// Connects to `pane`'s Holder. The PTY keeps the size people's Clients set: shown in the
    /// caller's UI and on ToH at once, either side may type (ADR-0001).
    pub fn open(roots: Roots, pane: &Pane, out: Out) -> Result<Engine> {
        let conn = HolderConn::connect(&pane.socket)?;
        let size = conn.size();
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
            idle_at: None,
            no_checkin_since: None,
            answered_until: None,
            history: HistoryWorker::start(wake.clone()),
            model: None,
            effort: None,
            context_tokens: None,
            hook_mode: None,
            screen_mode: None,
            info_sent: None,
            feedback: None,
            feedback_gone_since: None,
            looked_gen: u64::MAX,
            looked_at: None,
            progress_sent: None,
            fatal: None,
            holder_id: pane.id.clone(),
            shell_pid: pane.shell_pid,
            conn,
            out,
            wake,
            fs,
            size,
            grown_at: std::cell::Cell::new(None),
            claude_pid: None,
            followed: None,
            parked_job: None,
            pane_lock: None,
            claude_seen_at: None,
            last_locate: None,
            checkins_seen: HashSet::new(),
            pf: None,
            session_id: None,
            ready_sent: false,
            tailer: None,
            state: State::Starting,
            raw_since: None,
            turns: TurnTracker::new(),
            library: Library::load(layout::library_dir()),
            analyzer: Analyzer::from_env(),
            analysis: None,
            analyzed: HashSet::new(),
            analyses: VecDeque::new(),
            choice_seq: 0,
            rewind: None,
            rewind_seq: 0,
            rewound: None,
            fork: None,
            announced: None,
            startup_checked: None,
            startup_last: None,
            follow_ups: VecDeque::new(),
            queue: Vec::new(),
            marks: Marks::default(),
            resolved: HashSet::new(),
            procfile_warned: false,
            exited: false,
            quiet: false,
            input: None,
            deferred: VecDeque::new(),
            abort_point: false,
        })
    }

    fn emit(&self, v: Value) {
        if self.quiet {
            return;
        }
        if crate::log::enabled() {
            let s = v.to_string();
            let short = if v["type"] == "block" {
                format!("block {} {}", v["kind"], v["uuid"])
            } else {
                s.chars().take(400).collect()
            };
            crate::clog!("out {short}");
            // Events carrying a screen: the whole of it, for looking into what was not recognized.
            if let Some(lines) = v["lines"].as_array() {
                crate::log::screen(lines.iter().filter_map(Value::as_str));
            }
        }
        self.out.send(&v);
    }

    fn locate_claude(&mut self) {
        if self.claude_pid.is_some() {
            return;
        }
        // A check-in from this Holder (its SessionStart ran): look now, not at the next scan.
        let checked_in = hook::checkin_pids(&self.state_dir, &self.holder_id)
            .into_iter()
            .fold(false, |new, pid| self.checkins_seen.insert(pid) || new);
        if !checked_in
            && self
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
        let Some(pid) = self.followed.or(self.claude_pid) else {
            return;
        };
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
            || self
                .pf
                .as_ref()
                .and_then(|p| p.session_id.as_deref())
                .is_some_and(|s| s != c.session_id)
        {
            let since = *self.no_checkin_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= CHECKIN_GRACE && self.fatal.is_none() {
                self.fatal = hook::checkin_problem(
                    Some(&c),
                    self.pf.as_ref().and_then(|p| p.session_id.as_deref()),
                )
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
            self.fatal = Some(format!(
                "ccrpc 插件报到的 transcript 不在 Claude Code 的 projects 目录里：{}",
                path.display()
            ));
            return;
        }
        let history = path.exists();
        self.switch_to(c.session_id, &path, history, HashSet::new());
    }

    /// The Pane's claude moved its conversation to the background (agents page, docs/probes.md):
    /// it stays as the front end and shows the background claude running the job. Hooks, Process
    /// File and transcript are that claude's from then on (`session_changed` to its Session); keys
    /// still go to the Pane. A background claude that is gone (stopped) stays followed until its
    /// job runs again (opened again: another pid, the same Session).
    fn follow_moved(&mut self) {
        let Some(pane_pid) = self.claude_pid else {
            return;
        };
        let (pid, pf, sid, path) =
            match locate::moved(&self.roots, &self.state_dir, self.parked_job.as_deref()) {
                locate::Moved::In { pf, checkin } => {
                    let Some(pid) = pf.pid else { return };
                    if self.followed == Some(pid) {
                        return;
                    }
                    (
                        Some(pid),
                        Some(pf),
                        checkin.session_id,
                        PathBuf::from(checkin.transcript_path),
                    )
                }
                // Not seen so far: the conversation came back to the Pane's claude.
                locate::Moved::No if self.followed.is_some() => {
                    let Some(c) = hook::read_checkin(&self.state_dir, pane_pid) else {
                        return;
                    };
                    let pf = procfile::read(&self.roots, pane_pid);
                    (None, pf, c.session_id, PathBuf::from(c.transcript_path))
                }
                _ => return,
            };
        if !locate::is_transcript_path(&self.roots, &path) {
            return;
        }
        crate::clog!(
            "follow {} → claude {} session {sid}",
            self.followed.unwrap_or(pane_pid),
            pid.unwrap_or(pane_pid)
        );
        // What came before is reported first: the old transcript's last entries, the hook events
        // still queued on the old socket.
        loop {
            let evs = self.tailer.as_mut().map(|t| t.poll()).unwrap_or_default();
            for ev in evs {
                self.on_tx(ev);
            }
            if !self.tailer.as_ref().is_some_and(Tailer::behind) {
                break;
            }
        }
        if let Some(old) = self.hooks.take() {
            while let Some(m) = old.try_recv() {
                self.on_hook(m);
            }
            if self.followed.is_none() {
                self.pane_lock = old.into_lock();
            }
        }
        // The new Session's transcript starts with a copy of the one the Pane's claude ran (the
        // same job opened again is the same Session: nothing to switch).
        let mut copied = HashSet::new();
        if pid.is_none() {
            self.pane_lock = None;
        } else if self.followed.is_none()
            && let Some(c) = hook::read_checkin(&self.state_dir, pane_pid)
        {
            let old = PathBuf::from(c.transcript_path);
            if locate::is_transcript_path(&self.roots, &old) {
                copied = transcript::entry_uuids(&old);
            }
        }
        match HookServer::bind(&self.state_dir, pid.unwrap_or(pane_pid), self.wake.clone()) {
            Ok(h) => self.hooks = Some(h),
            Err(e) => {
                self.fatal = Some(format!("{e:#}"));
                return;
            }
        }
        self.followed = pid;
        if pf.is_some() {
            self.pf = pf;
        }
        // A turn running when the conversation moved goes on there.
        self.turns.moved();
        // Seen as it happens, the new transcript is read live: past the copy it holds only what
        // the background claude wrote since (the rest of a reply moved mid-turn). Found later
        // (attach), it is history.
        let history = path.exists() && (copied.is_empty() || !self.ready_sent);
        self.switch_to(sid, &path, history, copied);
    }

    /// Switches to the Session `sid` whose transcript is `path`. A transcript that already existed
    /// is read once as history: its blocks are not emitted, but prompts / enqueues still confirm
    /// commands and pending tools are known. One created afterwards (a new Session's first prompt,
    /// after /clear) is live and read by `tick`.
    /// `copied`: the uuids of the entries it starts with, copied from the Session it continues;
    /// they produce no events.
    fn switch_to(&mut self, sid: String, path: &Path, history: bool, copied: HashSet<String>) {
        if self.session_id.as_deref() == Some(sid.as_str()) && self.tailer.is_some() {
            return;
        }
        // A Fork's first Session (from its check-in or its SessionStart) starts with a copy of
        // the Origin Session's entries.
        let copied = if copied.is_empty() && self.session_id.is_none() {
            self.fork_copied()
        } else {
            copied
        };
        let prev = self.session_id.replace(sid.clone());
        // Also the first one after `ready` (`new`: the plugin checks in after the startup screens).
        if prev.as_deref() != Some(sid.as_str()) && (prev.is_some() || self.ready_sent) {
            self.rewound = None;
            self.emit(json!({"type": "session_changed", "session_id": sid, "previous": prev}));
        }
        let mut t = Tailer::new(path);
        t.copied_from(copied);
        if history {
            loop {
                for ev in t.poll() {
                    match ev {
                        TxEvent::UserPrompt { uuid, text, images } => {
                            self.marks.prompt(uuid, text, images)
                        }
                        TxEvent::Enqueued { content } => self.marks.enqueue(content),
                        TxEvent::Block(b) if b.agent_id.is_none() && b.kind == BlockKind::Text => {
                            self.turns.note_history_text(b.text.clone());
                        }
                        TxEvent::Usage {
                            main: true,
                            model,
                            effort,
                            usage,
                            ..
                        } => {
                            self.note_main_usage(model.as_deref(), effort.as_deref(), &usage);
                        }
                        TxEvent::Setting { model, effort } => self.note_setting(model, effort),
                        _ => {}
                    }
                }
                if !t.behind() {
                    break;
                }
            }
        }
        // `get_messages` reads the history on its own thread: start now.
        self.history.warm(path);
        self.set_queue(t.queue());
        self.tailer = Some(t);
    }

    pub fn tick(&mut self) {
        if self.exited {
            return;
        }
        if self.conn.poll_closed() {
            self.set_exited();
            return;
        }
        self.size = self.conn.size();
        self.locate_claude();
        if let Some(pid) = self.claude_pid {
            if !::transcript::live::pid_exists(pid) {
                self.set_exited();
                return;
            }
            if let Some((pf, job)) = procfile::read_with_job(&self.roots, pid) {
                self.parked_job = job.parked_job_id;
                if self.followed.is_none() {
                    self.pf = Some(pf);
                }
            }
            self.follow_moved();
            if let Some(b) = self.followed
                && let Some(pf) = self
                    .parked_job
                    .as_deref()
                    .and_then(|j| procfile::job_process_at(&self.roots, b, j))
            {
                self.pf = Some(pf);
            }
        }
        self.follow_checkin();
        self.update_watches();
        let evs = self.tailer.as_mut().map(|t| t.poll()).unwrap_or_default();
        for ev in evs {
            self.on_tx(ev);
        }
        while let Some(m) = self.hooks.as_ref().and_then(HookServer::try_recv) {
            self.on_hook(m);
        }
        // A held hook Claude Code killed (answered in the dialog, timed out): the request, if
        // still on screen, is answered with keys from now on.
        let waiting = matches!(self.state, State::Waiting { .. });
        for r in &mut self.requests {
            if r.reply.as_ref().is_some_and(hook::peer_gone) {
                r.reply = None;
            }
            // Answered in the TUI (its hook is not killed then): the Session went on without it
            // for a while — not a flicker.
            if r.reply.is_some() {
                if waiting {
                    r.away_since = None;
                } else if r.away_since.get_or_insert_with(Instant::now).elapsed()
                    >= Duration::from_secs(1)
                {
                    r.reply = None;
                }
            }
        }
        if self.answered_until.is_some_and(|t| Instant::now() >= t) {
            self.answered_until = None;
        }
        self.update_state();
        let idle_since = self.idle_at.filter(|_| self.state == State::Idle);
        let out = self.turns.poll(Instant::now(), idle_since);
        self.apply_turns(out);
        self.refresh_announcement();
        self.watch_rewind();
        self.announce_requests();
        self.look_at_screen();
    }

    /// The permission mode: the mode line under the input box, else what the plugin last saw.
    fn mode(&self) -> Option<String> {
        if let Some(m) = &self.screen_mode {
            return Some(m.clone());
        }
        self.hook_mode.as_deref().map(|m| {
            match m {
                "default" => "manual",
                "acceptEdits" => "accept_edits",
                "bypassPermissions" => "bypass",
                "dontAsk" => "dont_ask",
                other => other,
            }
            .to_string()
        })
    }

    /// The feedback draft card on a new screen, judged only while idle: Claude Code hides it during
    /// a turn and a dialog covers it, while the draft stays. Reported when it appears or stands for
    /// another draft; gone only once the input box shows without it. With the `SendFeedback` call
    /// behind it: the transcript's last one, titled as the card (or starting so when the card cuts
    /// the title).
    fn see_feedback_draft(&mut self, s: &Screen) {
        if self.state != State::Idle {
            self.feedback_gone_since = None;
            return;
        }
        let Some(card) = recognize::feedback_draft(&s.lines) else {
            if recognize::input_box(&s.lines).is_some() {
                self.feedback_gone_since.get_or_insert_with(Instant::now);
            } else {
                self.feedback_gone_since = None;
            }
            return;
        };
        self.feedback_gone_since = None;
        let squeeze = |t: &str| -> String { t.chars().filter(|c| !c.is_whitespace()).collect() };
        let cut = card.title.ends_with('…');
        let want = squeeze(card.title.trim_end_matches('…'));
        let call = self
            .tailer
            .as_ref()
            .and_then(|t| t.last_feedback())
            .filter(|(_, input)| {
                let title = squeeze(input["title"].as_str().unwrap_or_default());
                !want.is_empty() && (title == want || (cut && title.starts_with(&want)))
            });
        let id = call.map(|(id, _)| id.clone());
        // The same card: reported again only when its call is found now. A later draft (queued
        // behind it) or a new transcript (/clear) does not take back the details found before.
        if self.feedback.as_ref().is_some_and(|d| {
            d["shown"].as_str() == Some(card.title.as_str())
                && (id.is_none() || d["tool_use_id"].as_str() == id.as_deref())
        }) {
            return;
        }
        let draft = json!({
            "title": call.and_then(|(_, i)| i["title"].as_str()).unwrap_or(&card.title),
            "tool_use_id": id,
            "input": call.map(|(_, i)| i),
            "shown": card.title,
        });
        crate::clog!("recognize feedback draft {:?}", draft["title"]);
        self.emit(json!({"type": "feedback_draft", "draft": public_draft(&draft)}));
        self.feedback = Some(draft);
    }

    /// The input box has shown without the card for a second while idle: `feedback_draft` null.
    fn feedback_draft_gone(&mut self) {
        if self.state == State::Idle
            && self.feedback.is_some()
            && self
                .feedback_gone_since
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(1))
        {
            self.feedback = None;
            self.feedback_gone_since = None;
            self.emit(json!({"type": "feedback_draft", "draft": null}));
        }
    }

    /// Dismisses the feedback draft card (its `0`; the draft stays in `/feedback` until it expires).
    /// Only while idle (the card is hidden during a turn), with the card on a fresh Snapshot and
    /// the empty input box under it holding the cursor: there the key is the card's, anywhere else
    /// it would be typed. Done once the card is gone (or shows another draft) and the box is still
    /// empty.
    fn dismiss_feedback_draft(&mut self) -> CmdResult {
        self.require(self.state == State::Idle, "not_idle")?;
        let mut s = self.fresh_screen()?;
        let Some(mut card) = recognize::feedback_draft(&s.lines) else {
            return err("no_feedback_draft", "屏幕上没有反馈草稿卡片");
        };
        // A short Pane pushes the input box off the screen: look again at WORK_SIZE.
        if recognize::input_empty_below(&s, card.bottom).is_none() && self.grow() {
            self.grow_settle();
            s = self.fresh_screen()?;
            card = recognize::feedback_draft(&s.lines)
                .ok_or(("no_feedback_draft", "屏幕上没有反馈草稿卡片".to_string()))?;
        }
        match recognize::input_empty_below(&s, card.bottom) {
            Some(true) => {}
            Some(false) => return err("input_not_empty", "输入框里有字，0 会被当作输入"),
            None => {
                return err(
                    "unrecognized_screen",
                    "卡片下方看不到输入框（窗格太矮），没有按键",
                );
            }
        }
        self.keys(b"0")?;
        let title = card.title.clone();
        let done = self.wait_screen(
            Duration::from_secs(3),
            |_, s| match recognize::feedback_draft(&s.lines) {
                Some(c) if c.title == title => None,
                other => Some(other.is_none()),
            },
        );
        let Some((gone, after)) = done else {
            return err("timeout", "按 0 后反馈草稿卡片仍在");
        };
        if recognize::input_text(&after).is_none_or(|t| !t.is_empty()) {
            return err(
                "input_not_empty",
                "按 0 后输入框里多了字，卡片没有收到这个键",
            );
        }
        if gone && self.feedback.take().is_some() {
            self.feedback_gone_since = None;
            self.emit(json!({"type": "feedback_draft", "draft": null}));
        }
        Ok(json!({}))
    }

    fn session_info(&self) -> Value {
        json!({"mode": self.mode(), "model": self.model, "effort": self.effort, "context_tokens": self.context_tokens})
    }

    /// On a new screen: `session_info` when mode / model / context changed, `progress` while
    /// busy (the spinner line; at most once a second unless its verb changed).
    fn look_at_screen(&mut self) {
        let gen_now = self.conn.generation();
        // A spinner redraws the screen ~10 times a second: look at most every 300ms.
        let fresh = gen_now != self.looked_gen
            && self
                .looked_at
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(300));
        if fresh && let Ok(s) = self.screen() {
            if let Some(m) = recognize::permission_mode(&s.lines) {
                self.screen_mode = Some(m.name().to_string());
            }
            if let Some(e) = recognize::effort(&s.lines) {
                self.effort = Some(e);
            }
            self.see_feedback_draft(&s);
        }
        self.feedback_draft_gone();
        let info = self.session_info();
        if self.info_sent.as_ref() != Some(&info)
            && (info["mode"].is_string() || info["model"].is_string())
        {
            let mut v = info.clone();
            v["type"] = json!("session_info");
            self.emit(v);
            self.info_sent = Some(info);
        }
        if !fresh {
            return;
        }
        self.looked_gen = gen_now;
        self.looked_at = Some(Instant::now());
        if self.state != State::Busy {
            self.progress_sent = None;
            return;
        }
        let Some(sp) = self
            .screen()
            .ok()
            .and_then(|s| recognize::spinner(&s.lines))
        else {
            return;
        };
        let send = match &self.progress_sent {
            None => true,
            Some((text, at)) => {
                *text != sp.text
                    && (at.elapsed() >= Duration::from_secs(1)
                        || text.split('…').next() != Some(sp.verb.as_str()))
            }
        };
        if send {
            self.progress_sent = Some((sp.text.clone(), Instant::now()));
            self.emit(json!({"type": "progress", "text": sp.text, "verb": sp.verb,
                "elapsed_s": sp.elapsed_s, "tokens": sp.tokens}));
        }
    }

    /// File changes that matter wake the loop: Process Files, and the transcript with its
    /// Subagent / Workflow / tool-results files.
    fn update_watches(&mut self) {
        use notify::RecursiveMode::{NonRecursive, Recursive};
        let sessions = self.roots.sessions_dir();
        let checkins = self.state_dir.join("sessions");
        let main = self.tailer.as_ref().map(|t| t.main_path().to_path_buf());
        let mut paths = vec![(sessions.as_path(), NonRecursive)];
        // Until claude is found: its check-in wakes the loop (`locate_claude`).
        if self.claude_pid.is_none() {
            paths.push((checkins.as_path(), NonRecursive));
        }
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
        ) && let Some(Announced::Request(id)) = self.announced.take()
            && self.open_request(&id).is_none()
        {
            self.resolved.insert(id);
        }
        self.announced = None;
        // Nothing of the main line waits for an answer once the Session is idle (an Escape kills
        // waiting hooks). A background Subagent's request may show its dialog only now.
        if s == State::Idle {
            self.idle_at = Some(Instant::now());
            // The screen may have settled before the Process File said idle: look at it again
            // (the feedback draft card is judged only while idle).
            self.looked_gen = u64::MAX;
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
        crate::clog!(
            "state {:?} (raw {:?})",
            s,
            self.raw_since.as_ref().map(|r| &r.0)
        );
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
                            .requests
                            .iter()
                            .any(|r| r.tool_name == "AskUserQuestion");
                        if has_ask {
                            WaitKind::Question
                        } else if since.elapsed() < CLASSIFY_GRACE || self.answered_until.is_some()
                        {
                            return; // the plugin's PermissionRequest may still be on its way
                        } else if self.pending_ask().is_some() {
                            // Asked while no ccrpc held the hook (attached later): answered on the screen.
                            WaitKind::Question
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

    /// Claude Code's queue changed: keep the person's part, and tell the caller.
    fn set_queue(&mut self, items: &[String]) {
        let mine: Vec<String> = items
            .iter()
            .filter(|t| {
                let t = t.trim_start();
                !t.starts_with("<task-notification>") && !t.contains("<teammate-message")
            })
            .cloned()
            .collect();
        if mine != self.queue {
            self.queue = mine;
            self.emit(json!({"type": "queue", "items": self.queue}));
        }
    }

    fn get_state(&self) -> Value {
        let mut v = self.state.to_json();
        let request = self.announced.as_ref().map(|a| match a {
            Announced::Choice { event, .. } => event.clone(),
            Announced::Request(id) => match self.open_request(id) {
                Some(r) => json!({"request_id": r.id, "tool_name": r.tool_name, "input": r.input,
                    "agent_id": r.agent_id}),
                None => json!({"waiting_for": id}),
            },
            Announced::Dialog(waiting_for) => json!({"waiting_for": waiting_for}),
        });
        // An announced rewind is the request (its confirm page waits in the TUI).
        let rewind = self
            .rewind
            .as_ref()
            .filter(|r| r.id.is_some())
            .map(|r| r.event.clone());
        v["request"] = rewind.or(request).unwrap_or(Value::Null);
        v["holder_id"] = json!(self.holder_id);
        v["session_id"] = json!(self.session_id);
        v["cwd"] = json!(self.pf.as_ref().and_then(|p| p.cwd.clone()));
        v["follow_up_queue"] = json!(self.follow_ups.len());
        v["follow_ups"] = json!(self.follow_up_texts());
        v["queue"] = json!(self.queue);
        v["size"] = json!([self.size.0, self.size.1]);
        let info = self.session_info();
        for k in ["mode", "model", "effort", "context_tokens"] {
            v[k] = info[k].clone();
        }
        v["feedback_draft"] = self.feedback.as_ref().map_or(Value::Null, public_draft);
        v
    }

    /// A page of the history, from the history thread (events keep flowing while it reads).
    fn get_messages(&mut self, q: history::Query) -> CmdResult {
        // The Session may change while the page is read (/clear): read the new one then.
        for _ in 0..3 {
            let Some(path) = self.tailer.as_ref().map(|t| t.main_path().to_path_buf()) else {
                return Ok(json!({"blocks": [], "cursor": 0, "first": null, "more": false}));
            };
            let rx = self.history.page(&path, history::Query { ..q });
            let mut got = None;
            // It writes nothing to the Pane: an `abort` meanwhile need not wait for the page.
            self.wait_after_keys(Duration::from_secs(120), |_| {
                if got.is_none() {
                    got = rx.try_recv().ok();
                }
                got.is_some()
            });
            let Some(mut v) = got else {
                return err("timeout", "120 秒内没读完 transcript");
            };
            if self.tailer.as_ref().is_some_and(|t| t.main_path() == path) {
                if let Some(m) = v.as_object_mut() {
                    m.remove("path");
                }
                return Ok(v);
            }
        }
        err("timeout", "读的期间 Session 一直在换")
    }

    pub fn execute(&mut self, c: &Command) -> CmdResult {
        match c {
            Command::Prompt { input } => self.send_input(input),
            Command::CancelFollowUp { index } => self.cancel_follow_up(*index),
            Command::SetMode { mode } => self.set_mode(*mode),
            Command::GetTasks => Ok(json!({"tasks": self.tasks()})),
            Command::StopTask { task_id } => self.stop_task(task_id),
            Command::DismissFeedbackDraft => self.dismiss_feedback_draft(),
            Command::AnswerChoice { request_id, option } => self.answer_choice(request_id, option),
            Command::Rewind { uuid } => self.rewind(uuid),
            Command::AnswerRewind { request_id, option } => {
                self.answer_rewind_cmd(request_id, *option)
            }
            Command::Steer { input } => self.steer_input(input),
            Command::FollowUp { input } => {
                self.require(true, "")?;
                // Checked now, not only when sent after the turn.
                let input = Self::checked_input(input)?;
                self.follow_ups.push_back(input);
                Ok(json!({"queued": self.follow_ups.len()}))
            }
            Command::Abort => self.abort(),
            Command::SendNow => self.send_now(),
            Command::AnswerPermission {
                request_id,
                decision,
                message,
            } => self.answer_permission(request_id, decision, message.as_deref()),
            Command::AnswerQuestion {
                request_id,
                answers,
            } => self.answer_question(request_id, answers),
            Command::ChatAboutQuestion {
                request_id,
                answers,
            } => self.chat_about_question(request_id, answers),
            Command::Slash { command } => self.slash(command),
            Command::SetModel { model } => self.slash(&format!("model {model}")),
            Command::SetEffort { level } => self.slash(&format!("effort {level}")),
            Command::Compact { instructions } => match instructions {
                Some(i) => self.slash(&format!("compact {i}")),
                None => self.slash("compact"),
            },
            Command::Clear => self.slash("clear"),
            Command::SendKeys { keys } => self.send_keys(keys),
            Command::GetState => Ok(self.get_state()),
            Command::GetMessages {
                since,
                before,
                limit,
                subagents,
            } => self.get_messages(history::Query {
                since: since.unwrap_or(0),
                before: *before,
                limit: *limit,
                subagents: *subagents,
            }),
            Command::Exit {
                close_pane,
                stop_tasks,
            } => self.exit(*close_pane, *stop_tasks),
            Command::GetScreen => {
                let s = self.screen().map_err(|e| ("io", format!("{e:#}")))?;
                Ok(json!({"lines": s.lines, "cursor": [s.cursor.0, s.cursor.1]}))
            }
            Command::GetToolResult { tool_use_id } => {
                let found = self
                    .tailer
                    .as_ref()
                    .and_then(|t| t.tool_result(tool_use_id));
                match found {
                    Some((content, persisted)) => {
                        Ok(json!({"content": content, "persisted": persisted}))
                    }
                    None => err(
                        "unknown_request",
                        format!("找不到 {tool_use_id} 的工具结果"),
                    ),
                }
            }
            Command::GetImage { uuid, index } => {
                match self.tailer.as_ref().and_then(|t| t.image(uuid, *index)) {
                    Some((media_type, data)) => Ok(json!({"media_type": media_type, "data": data})),
                    None => err(
                        "unknown_request",
                        format!("找不到 {uuid} 的第 {index} 张图片"),
                    ),
                }
            }
        }
    }

    fn ready(&mut self) {
        self.ready_sent = true;
        let version = self.pf.as_ref().and_then(|p| p.version.clone());
        crate::clog!(
            "ccrpc {} holder={} session={} claude={} size={}x{}",
            env!("CARGO_PKG_VERSION"),
            self.holder_id,
            self.session_id.as_deref().unwrap_or("-"),
            version.as_deref().unwrap_or("-"),
            self.size.0,
            self.size.1
        );
        let mut v = self.state.to_json();
        v["type"] = json!("ready");
        v["protocol"] = json!(PROTOCOL);
        v["holder_id"] = json!(self.holder_id);
        v["session_id"] = json!(self.session_id);
        v["cwd"] = json!(self.pf.as_ref().and_then(|p| p.cwd.clone()));
        v["claude_version"] = json!(version);
        v["size"] = json!([self.size.0, self.size.1]);
        v["queue"] = json!(self.queue);
        // Seen by the first look, before events went out.
        v["feedback_draft"] = self.feedback.as_ref().map_or(Value::Null, public_draft);
        let info = self.session_info();
        for k in ["mode", "model", "effort", "context_tokens"] {
            v[k] = info[k].clone();
        }
        let mut warnings = Vec::new();
        if let Some(ver) = &version
            && !TESTED_CLAUDE_VERSIONS.contains(&ver.as_str())
        {
            warnings.push(format!(
                "Claude Code {ver} 不是实测过的版本，认不出的界面会交给分析器或报 unrecognized"
            ));
        }
        v["warnings"] = json!(warnings);
        self.emit(v);
    }

    /// Runs until stdin closes.
    pub fn run(mut self, input: Receiver<String>) -> Result<()> {
        self.input = Some(input);
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
            // Between commands only: a command in progress is never cut off.
            if startup::terminated() {
                crate::clog!("SIGTERM: detaching");
                if self.fork_unfinished() {
                    self.end_fork_pane();
                }
                break;
            }
            // `new --fork`: nothing that writes runs before the Fork is made. Disconnected
            // before it is: its claude and Pane end (a Fork not made is not left behind).
            if self.fork_holding() {
                let seen = self.wake.seen();
                self.tick();
                self.step_fork();
                if self.fork_holding() {
                    if !self.take_input_while_forking() {
                        self.end_fork_pane();
                        break;
                    }
                    self.wake.wait(seen, IDLE_TICK);
                }
                continue;
            }
            // Lines read while the last command ran come first, in order.
            if let Some(line) = self.deferred.pop_front() {
                self.handle_line(&line);
                continue;
            }
            // Taken before looking at stdin: a line arriving now still wakes the wait below.
            let seen = self.wake.seen();
            let next = self
                .input
                .as_ref()
                .map_or(Err(TryRecvError::Disconnected), Receiver::try_recv);
            match next {
                Ok(line) => self.handle_line(&line),
                Err(TryRecvError::Empty) => {
                    self.tick();
                    // A SIGTERM during the tick: no queued prompt goes out after it.
                    if !startup::terminated() {
                        self.drain_follow_ups();
                    }
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
        // A turn already running is adopted: its Stop (whatever its prompt id) ends it. The
        // Process File says so even while its waiting is not classified yet.
        let raw = self.pf.as_ref().and_then(|p| p.status.as_deref());
        if matches!(self.state, State::Busy | State::Waiting { .. })
            || matches!(raw, Some("busy" | "waiting"))
        {
            self.turns.adopt_running();
        } else {
            self.turns.idle_at_connect();
        }
        self.announced = None;
        self.startup_checked = None;
        self.startup_last = None;
    }

    /// While a command waits: lines from stdin that neither write to the Pane nor wait (reads, the
    /// follow-up queue) and bad lines are answered at once; at an abort point (the command pressed
    /// its last key) an `abort` runs now, once there is a turn to abort. Never ahead of a line
    /// that came earlier: once one waits for the command to end, the ones after it wait too.
    /// Everything else runs after the command, in order. Only the `abort` waits (up to 10 s for
    /// the turn to stop), nesting one level: `abort_point` is off meanwhile, so its own wait
    /// answers reads but runs no second abort.
    pub(super) fn serve_input(&mut self) {
        let mut fresh = Vec::new();
        if let Some(rx) = &self.input {
            while let Ok(line) = rx.try_recv() {
                fresh.push(line);
            }
        }
        for line in fresh {
            let now = self.deferred.is_empty()
                && !matches!(rpc::parse_line(&line), Ok(r) if !serves_in_wait(&r.command));
            if now {
                self.handle_line(&line);
            } else {
                self.deferred.push_back(line);
            }
        }
        if self.abort_point
            && !self.fork_unfinished()
            && matches!(self.state, State::Busy | State::Waiting { .. })
            && self
                .deferred
                .front()
                .is_some_and(|l| rpc::parse_line(l).is_ok_and(|r| r.command == Command::Abort))
            && let Some(line) = self.deferred.pop_front()
        {
            // Its own wait is no abort point: a second abort waits for it.
            self.abort_point = false;
            self.handle_line(&line);
            self.abort_point = true;
        }
    }

    fn handle_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        match rpc::parse_line(line) {
            Ok(Request { id, command }) => {
                let name = rpc::command_name(&command);
                crate::clog!("command {name} id={}", id.as_deref().unwrap_or("-"));
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
