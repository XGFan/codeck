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
use crate::history::{self, HistoryWorker};
use crate::hook::{self, HookMsg, HookServer};
use crate::layout::{self, Layout, LayoutKind, Library, Meaning};
use crate::locate::{self, Roots};
use crate::procfile::{self, ProcessFile};
use crate::pty::{self, HolderConn, Owner, key};
use crate::recognize::{self, PermissionDialog};
use crate::rpc::{self, Answer, Command, Decision, Out, Request, RpcError};
use crate::screen::Screen;
use crate::transcript::{self, BgTask, BlockKind, Tailer, TaskKind, TxEvent};
use crate::wake::{FsWatch, Wake};

mod announce;
mod drive;
mod requests;
mod startup;
mod tasks;
mod turn;

use self::announce::Announced;
use self::requests::{HookPrompt, HookRequest, PreTool};
pub(crate) use self::startup::toh_command;
pub use self::startup::{Opts, cmd_attach, cmd_new, resolve_target};
use self::turn::{Marks, TurnStats};

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
/// PTY size ccrpc sets when it takes the size (ADR-0001).
pub const DEFAULT_SIZE: (u16, u16) = (160, 48);
/// Claude Code versions the built-in layouts were probed on.
pub const TESTED_CLAUDE_VERSIONS: &[&str] = &["2.1.316", "2.1.317"];
/// At most this many analyses an hour.
const MAX_ANALYSES_PER_HOUR: usize = 10;

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
    /// Shown in the caller's UI and a ToH Client at once: no Resize, no takeover.
    co_display: bool,
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
    /// Screen generation last looked at for the mode line and the spinner, and when.
    looked_gen: u64,
    looked_at: Option<Instant>,
    /// `progress` last sent: (text, when).
    progress_sent: Option<(String, Instant)>,
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
    /// `ready` went out: a sessionId learned from now on is announced (`session_changed`).
    ready_sent: bool,
    tailer: Option<Tailer>,
    state: State,
    /// Raw `(status, waitingFor)` last read, with when it was first seen.
    raw_since: Option<((String, Option<String>), Instant)>,
    /// A turn is running (opened by UserPromptSubmit, or running at connect) and has not ended.
    turn_active: bool,
    last_text: Option<String>,
    turn: TurnStats,
    prices: ::transcript::pricing::PriceTable,
    library: Library,
    analyzer: Option<Analyzer>,
    /// The analysis in flight: dialog signature and where its result arrives.
    analysis: Option<(String, Receiver<std::result::Result<Layout, String>>)>,
    /// Dialog signatures already analyzed (never twice).
    analyzed: HashSet<String>,
    analyses: VecDeque<Instant>,
    /// Numbers the choices announced (`choice:N`).
    choice_seq: u64,
    /// The Holder reports size owners (not an older Holder).
    holder_reports_owner: bool,
    /// What went out for the current waiting.
    announced: Option<Announced>,
    startup_checked: Option<Instant>,
    startup_last: Option<Vec<String>>,
    /// Queued follow-ups: text and image paths.
    follow_ups: VecDeque<(String, Vec<String>)>,
    /// What the person typed (or a steer sent) waiting in Claude Code's own queue, oldest first:
    /// the transcript's queue minus task notifications and Teammate messages.
    queue: Vec<String>,
    /// The next turn's Stop, come while this one's still waits for the transcript (claude fires
    /// both Stops before writing either turn): handled once this turn has ended.
    deferred_stop: Option<HookMsg>,
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
            // Shown alongside a person: nobody's use excludes ccrpc.
            _ if opts.co_display => {}
            Owner::Other(client) if !opts.force => {
                conn.detach();
                bail!("Session 正被 {client} 使用，加 --force 强制连接");
            }
            Owner::Unknown => {
                eprintln!("ccrpc: Holder {holder_id} 不报告尺寸所有者，无法确认是否有人在用")
            }
            _ => {}
        }
        // Co-display leaves the size to the people's Clients.
        let size = if opts.co_display {
            (cols, rows)
        } else {
            conn.claim_size(opts.size.0, opts.size.1)?;
            opts.size
        };
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
            co_display: opts.co_display,
            history: HistoryWorker::start(wake.clone()),
            model: None,
            effort: None,
            context_tokens: None,
            hook_mode: None,
            screen_mode: None,
            info_sent: None,
            looked_gen: u64::MAX,
            looked_at: None,
            progress_sent: None,
            fatal: None,
            holder_id: holder_id.to_string(),
            shell_pid: meta.child_pid,
            tty,
            conn,
            out,
            wake,
            fs,
            size,
            taken_over: false,
            last_size_check: Instant::now(),
            claude_pid: None,
            claude_seen_at: None,
            last_locate: None,
            pf: None,
            session_id: None,
            ready_sent: false,
            tailer: None,
            state: State::Starting,
            raw_since: None,
            turn_active: false,
            last_text: None,
            turn: TurnStats::default(),
            prices: ::transcript::pricing::PriceTable::embedded(),
            library: Library::load(layout::library_dir()),
            analyzer: Analyzer::from_env(),
            analysis: None,
            analyzed: HashSet::new(),
            analyses: VecDeque::new(),
            choice_seq: 0,
            holder_reports_owner,
            announced: None,
            startup_checked: None,
            startup_last: None,
            follow_ups: VecDeque::new(),
            queue: Vec::new(),
            deferred_stop: None,
            marks: Marks::default(),
            resolved: HashSet::new(),
            procfile_warned: false,
            exited: false,
            quiet: false,
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
        // Also the first one after `ready` (`new`: the plugin checks in after the startup screens).
        if prev.as_deref() != Some(sid.as_str()) && (prev.is_some() || self.ready_sent) {
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
        self.check_takeover();
        self.locate_claude();
        if let Some(pid) = self.claude_pid {
            if !::transcript::live::pid_exists(pid) {
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
        }
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
        v["request"] = request.unwrap_or(Value::Null);
        v["holder_id"] = json!(self.holder_id);
        v["session_id"] = json!(self.session_id);
        v["cwd"] = json!(self.pf.as_ref().and_then(|p| p.cwd.clone()));
        v["follow_up_queue"] = json!(self.follow_ups.len());
        v["follow_ups"] = json!(self.follow_up_texts());
        v["queue"] = json!(self.queue);
        v["taken_over"] = json!(self.taken_over);
        let info = self.session_info();
        for k in ["mode", "model", "effort", "context_tokens"] {
            v[k] = info[k].clone();
        }
        v["co_display"] = json!(self.co_display);
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
            self.wait_until(Duration::from_secs(120), |_| {
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
        let reads = matches!(
            c,
            Command::GetState
                | Command::GetMessages { .. }
                | Command::GetScreen
                | Command::GetToolResult { .. }
                | Command::GetImage { .. }
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
            Command::Steer { text, images } => self.steer(text, images),
            Command::FollowUp { text, images } => {
                self.require(true, "")?;
                Self::clean_text(text)?;
                // Checked now, not only when sent after the turn.
                let images = Self::image_paths(images)?;
                self.follow_ups.push_back((text.clone(), images));
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
            "ccrpc {} holder={} session={} claude={} co_display={}",
            env!("CARGO_PKG_VERSION"),
            self.holder_id,
            self.session_id.as_deref().unwrap_or("-"),
            version.as_deref().unwrap_or("-"),
            self.co_display
        );
        let mut v = self.state.to_json();
        v["type"] = json!("ready");
        v["protocol"] = json!(PROTOCOL);
        v["holder_id"] = json!(self.holder_id);
        v["session_id"] = json!(self.session_id);
        v["cwd"] = json!(self.pf.as_ref().and_then(|p| p.cwd.clone()));
        v["claude_version"] = json!(version);
        v["size"] = json!([self.size.0, self.size.1]);
        v["taken_over"] = json!(self.taken_over);
        v["co_display"] = json!(self.co_display);
        v["queue"] = json!(self.queue);
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
            // Between commands only: a command in progress is never cut off.
            if startup::terminated() {
                crate::clog!("SIGTERM: detaching");
                break;
            }
            // Taken before looking at stdin: a line arriving now still wakes the wait below.
            let seen = self.wake.seen();
            match input.try_recv() {
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
        self.turn_active = matches!(self.state, State::Busy | State::Waiting { .. })
            || matches!(raw, Some("busy" | "waiting"));
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
