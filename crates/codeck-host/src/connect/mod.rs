//! Connect state held by the Host (ADR-0011; with `HostConfig::track_connect`): every connection's ccrpc lines are
//! folded into a [`ConnState`] that all of the Host's remote clients share, and they send ccrpc commands through the
//! Host ([`ConnectHub::command`]), which gives them ids `h<n>` and hands back the response. The desktop app's window
//! keeps doing this in its own store (`src/features/connect`) and adopts the Host's connections (ADR-0012).

pub mod state;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use codeck_core::model::{AppError, ErrorCode};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::oneshot;

pub use self::state::ConnState;
use crate::Host;
use crate::ccrpc::relay::{self, Out, Sink};
use crate::commands::{CmdResult, internal};
use crate::events::{EventSink, HostEvent};

/// Writes one command line to a connection's ccrpc (`App::send`); a fake in tests.
pub type Writer = Arc<dyn Fn(&str, &str) -> relay::R<()> + Send + Sync>;

/// How long a client's command may wait for ccrpc's response (its longest command waits 120 s).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(150);
/// A connection's state goes out at most this often (the last change always does).
const THROTTLE: Duration = Duration::from_millis(100);
/// How long a new attach waits for the ended connections of its Holder to go (ccrpc detaches once its stdin
/// closes, and gets SIGTERM after five seconds).
const ENDED_CONN_WAIT: Duration = Duration::from_secs(10);

pub struct ConnectHub {
    inner: Mutex<Inner>,
    events: EventSink,
    writer: Writer,
    throttle: Duration,
    seq: AtomicU64,
    /// One `connect_shared` at a time: two clients connecting to one Holder get one connection.
    shared: tokio::sync::Mutex<()>,
    /// The latest `ccrpc ls --json --watch` list, or why there is none.
    ls: Mutex<Result<String, Option<String>>>,
    /// Who owns the one `ccrpc ls --json --watch`. Also held while a watch is started or stopped, so a keep loop
    /// that was just ended cannot start one afterwards.
    keep: Mutex<Keep>,
    stopped: AtomicBool,
}

#[derive(Default)]
struct Keep {
    /// Of the latest keep loop; a loop runs while its own is the latest and `on`.
    generation: u64,
    on: bool,
    /// The front end wants the watch: what comes back when the keep loop ends.
    wanted: bool,
}

#[derive(Default)]
struct Inner {
    conns: HashMap<String, Entry>,
    /// Commands the Host sent (id `h<n>`) whose response has not come yet.
    pending: HashMap<String, Pending>,
    /// Connections whose ccrpc exited: lines or a registration coming late do not bring them back.
    gone: HashSet<String>,
}

struct Entry {
    state: ConnState,
    last_emit: Option<Instant>,
    /// A trailing emit is due.
    scheduled: bool,
}

struct Pending {
    conn: String,
    command: String,
    request_id: Option<String>,
    /// The client's command waits for it; none for the Host's own `get_tasks`.
    waiter: Option<oneshot::Sender<Value>>,
}

fn exited_response(command: &str) -> Value {
    json!({
        "type": "response",
        "command": command,
        "success": false,
        "error": { "code": "exited", "message": "ccrpc 已退出" },
    })
}

impl ConnectHub {
    pub fn new(events: EventSink, writer: Writer) -> Arc<Self> {
        Self::with_throttle(events, writer, THROTTLE)
    }

    fn with_throttle(events: EventSink, writer: Writer, throttle: Duration) -> Arc<Self> {
        Arc::new(ConnectHub {
            inner: Mutex::default(),
            events,
            writer,
            throttle,
            seq: AtomicU64::new(0),
            shared: tokio::sync::Mutex::new(()),
            ls: Mutex::new(Err(None)),
            keep: Mutex::default(),
            stopped: AtomicBool::new(false),
        })
    }

    fn next_id(&self) -> String {
        format!("h{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// The sink for connections' ccrpc: lines and exits are folded here, then go to `events` as before, except
    /// the responses to the Host's own commands.
    pub fn sink(self: &Arc<Self>) -> Sink {
        let hub = self.clone();
        Arc::new(move |o| match o {
            Out::Line { conn, line } => {
                if hub.on_line(&conn, &line) {
                    (hub.events)(HostEvent::CcrpcLine { conn, line });
                }
            }
            Out::Exit(e) => {
                hub.on_exit(&e.conn, e.code, e.stderr.clone());
                (hub.events)(HostEvent::CcrpcExit {
                    conn: e.conn,
                    code: e.code,
                    stderr: e.stderr,
                });
            }
        })
    }

    /// A connection was just started (`holder_id` for `attach`): its state, before its first line.
    pub fn register(self: &Arc<Self>, conn: &str, holder_id: Option<String>) {
        let mut inner = self.inner.lock();
        if inner.gone.contains(conn) {
            return;
        }
        let e = inner
            .conns
            .entry(conn.to_string())
            .or_insert_with(|| Entry {
                state: ConnState::new(conn, None),
                last_emit: None,
                scheduled: false,
            });
        if e.state.holder_id.is_none() {
            e.state.holder_id = holder_id;
        }
        self.changed(&mut inner, conn);
    }

    /// A client is disconnecting the connection: its coming exit is deliberate.
    pub fn mark_disconnected(self: &Arc<Self>, conn: &str) {
        let mut inner = self.inner.lock();
        if let Some(e) = inner.conns.get_mut(conn) {
            e.state.disconnected = true;
            self.changed(&mut inner, conn);
        }
    }

    /// False when the line is the Host's own (the response to a command it sent and still waits for): not
    /// relayed.
    fn on_line(self: &Arc<Self>, conn: &str, line: &str) -> bool {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return true;
        };
        if v["type"] == "response" {
            return !v["id"]
                .as_str()
                .is_some_and(|id| self.on_response(id, v.clone()));
        }
        let mut inner = self.inner.lock();
        if inner.gone.contains(conn) {
            return true;
        }
        let e = inner
            .conns
            .entry(conn.to_string())
            .or_insert_with(|| Entry {
                state: ConnState::new(conn, None),
                last_emit: None,
                scheduled: false,
            });
        if e.state.apply(&v) {
            self.changed(&mut inner, conn);
        }
        drop(inner);
        // A failed Fork: ccrpc closes the Pane and exits, and the window that did not start it has nothing to show.
        if v["type"] == "fork" && !v["error"].is_null() {
            (self.events)(HostEvent::CcrpcClosing {
                conn: conn.to_string(),
            });
        }
        // As the desktop app does: the background tasks again once connected and after each turn.
        if matches!(v["type"].as_str(), Some("ready" | "turn_end")) {
            self.send_own(conn, "get_tasks");
        }
        true
    }

    /// The Host's own command (no one waits for it); its response is handled in `on_response`.
    fn send_own(self: &Arc<Self>, conn: &str, command: &str) {
        let id = self.next_id();
        self.inner.lock().pending.insert(
            id.clone(),
            Pending {
                conn: conn.to_string(),
                command: command.to_string(),
                request_id: None,
                waiter: None,
            },
        );
        let line = json!({ "type": command, "id": id }).to_string();
        let (hub, conn) = (self.clone(), conn.to_string());
        // Not on the connection's reader thread: a write may block until ccrpc reads.
        std::thread::spawn(move || {
            if (hub.writer)(&conn, &line).is_err() {
                hub.inner.lock().pending.remove(&id);
            }
        });
    }

    /// The response to a command the Host sent (`id` pending): handled here, true. Otherwise false.
    fn on_response(self: &Arc<Self>, id: &str, r: Value) -> bool {
        let mut inner = self.inner.lock();
        let Some(p) = inner.pending.remove(id) else {
            return false;
        };
        if r["success"] == true
            && let Some(e) = inner.conns.get_mut(&p.conn)
        {
            let changed = match p.command.as_str() {
                "get_tasks" => match r["data"]["tasks"].as_array() {
                    Some(t) => {
                        e.state.tasks = t.clone();
                        true
                    }
                    None => false,
                },
                // Answered: the card goes at once (ccrpc ends it on the tool's result), as on the desktop.
                "answer_permission" | "answer_question" | "answer_choice" => {
                    let before = e.state.requests.len();
                    e.state
                        .requests
                        .retain(|q| q["request_id"].as_str() != p.request_id.as_deref());
                    e.state.requests.len() != before
                }
                _ => false,
            };
            if changed {
                self.changed(&mut inner, &p.conn);
            }
        }
        drop(inner);
        // On the connection's reader thread, so before its exit event: the Session is being quit on purpose.
        if p.waiter.is_some() && p.command == "exit" && r["success"] == true {
            (self.events)(HostEvent::CcrpcClosing { conn: p.conn });
        }
        if let Some(w) = p.waiter {
            let _ = w.send(r);
        }
        true
    }

    fn on_exit(&self, conn: &str, code: Option<i32>, stderr: String) {
        let mut inner = self.inner.lock();
        inner.gone.insert(conn.to_string());
        let ids: Vec<String> = inner
            .pending
            .iter()
            .filter(|(_, p)| p.conn == conn)
            .map(|(id, _)| id.clone())
            .collect();
        let waiting: Vec<Pending> = ids
            .iter()
            .filter_map(|id| inner.pending.remove(id))
            .collect();
        // Exited before its first line or its registration (e.g. `attach` refused, only stderr): still said,
        // so a client learns why.
        let mut state = inner
            .conns
            .remove(conn)
            .map_or_else(|| ConnState::new(conn, None), |e| e.state);
        state.proc_exit(code, stderr);
        (self.events)(HostEvent::ConnectState(Box::new(state)));
        (self.events)(HostEvent::ConnectRemoved {
            conn: conn.to_string(),
        });
        drop(inner);
        for p in waiting {
            if let Some(w) = p.waiter {
                let _ = w.send(exited_response(&p.command));
            }
        }
    }

    /// The connection's state changed: out now, or once the throttle allows. Emitted under the lock, so
    /// snapshots go out in order (the event sink must not call back into the hub).
    fn changed(self: &Arc<Self>, inner: &mut Inner, conn: &str) {
        let Some(e) = inner.conns.get_mut(conn) else {
            return;
        };
        if e.scheduled {
            return;
        }
        let now = Instant::now();
        let since = e.last_emit.map(|t| now - t);
        match since {
            Some(since) if since < self.throttle => {
                e.scheduled = true;
                let (hub, conn, wait) = (self.clone(), conn.to_string(), self.throttle - since);
                std::thread::spawn(move || {
                    std::thread::sleep(wait);
                    let mut inner = hub.inner.lock();
                    if let Some(e) = inner.conns.get_mut(&conn) {
                        e.scheduled = false;
                        e.last_emit = Some(Instant::now());
                        (hub.events)(HostEvent::ConnectState(Box::new(e.state.clone())));
                    }
                });
            }
            _ => {
                e.last_emit = Some(now);
                (self.events)(HostEvent::ConnectState(Box::new(e.state.clone())));
            }
        }
    }

    /// Every connection's state, by connection id.
    pub fn states(&self) -> Vec<ConnState> {
        let mut v: Vec<_> = self
            .inner
            .lock()
            .conns
            .values()
            .map(|e| e.state.clone())
            .collect();
        v.sort_by(|a, b| a.conn.cmp(&b.conn));
        v
    }

    /// The newest connection to `holder_id` whose Session is still running and which no one is disconnecting.
    fn live_conn_for(&self, holder_id: &str) -> Option<String> {
        // `c<n>`, numbered as started.
        let seq = |conn: &str| conn[1..].parse::<u64>().unwrap_or(0);
        self.inner
            .lock()
            .conns
            .values()
            .map(|e| &e.state)
            .filter(|s| s.holder_id.as_deref() == Some(holder_id))
            .filter(|s| s.proc_exit.is_none() && !s.session_exited && !s.disconnected)
            .max_by_key(|s| seq(&s.conn))
            .map(|s| s.conn.clone())
    }

    /// The connections to `holder_id` whose Session ended: their ccrpc keeps running, attached, driving nothing.
    fn ended_conns_for(&self, holder_id: &str) -> Vec<String> {
        self.conns_for(holder_id, |s| s.session_exited && !s.disconnected)
    }

    /// The connections to `holder_id` being disconnected: their ccrpc is still attached for a moment.
    fn disconnecting_conns_for(&self, holder_id: &str) -> Vec<String> {
        self.conns_for(holder_id, |s| s.disconnected)
    }

    fn conns_for(&self, holder_id: &str, which: impl Fn(&ConnState) -> bool) -> Vec<String> {
        self.inner
            .lock()
            .conns
            .values()
            .map(|e| &e.state)
            .filter(|s| s.holder_id.as_deref() == Some(holder_id) && which(s))
            .map(|s| s.conn.clone())
            .collect()
    }

    /// Blocks until the ccrpc of each of `conns` has exited, at most `timeout`.
    fn wait_gone(&self, conns: &[String], timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline
            && conns
                .iter()
                .any(|c| self.inner.lock().conns.contains_key(c))
        {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sends `command` (an object with `type`) to the connection's ccrpc with an id of the Host's and returns
    /// ccrpc's response as it is; one that exits meanwhile answers `exited`.
    pub async fn command(&self, conn: &str, command: Value) -> CmdResult<Value> {
        let Value::Object(mut obj) = command else {
            return Err(relay::invalid("命令须是 JSON 对象"));
        };
        let Some(kind) = obj.get("type").and_then(Value::as_str).map(str::to_string) else {
            return Err(relay::invalid("命令缺少 type"));
        };
        let request_id = obj
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = self.next_id();
        obj.insert("id".into(), id.clone().into());
        let (tx, rx) = oneshot::channel();
        {
            let mut inner = self.inner.lock();
            if !inner.conns.contains_key(conn) {
                return Err(relay::invalid("没有这个连接，或 ccrpc 已退出"));
            }
            inner.pending.insert(
                id.clone(),
                Pending {
                    conn: conn.to_string(),
                    command: kind.clone(),
                    request_id,
                    waiter: Some(tx),
                },
            );
        }
        let (writer, c, line) = (
            self.writer.clone(),
            conn.to_string(),
            Value::Object(obj).to_string(),
        );
        let wrote = tokio::task::spawn_blocking(move || writer(&c, &line))
            .await
            .map_err(internal)
            .and_then(|r| r);
        if let Err(e) = wrote {
            self.inner.lock().pending.remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(COMMAND_TIMEOUT, rx).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(_)) => Ok(exited_response(&kind)),
            Err(_) => {
                self.inner.lock().pending.remove(&id);
                Err(relay::io(format!(
                    "ccrpc 没有在 {} 秒内回应 {kind}",
                    COMMAND_TIMEOUT.as_secs()
                )))
            }
        }
    }

    /// `ccrpc ls --json --watch` printed a list, or ended (why).
    pub fn set_ls(&self, ls: Result<String, String>) {
        *self.ls.lock() = ls.map_err(Some);
    }

    fn ls_entries(&self) -> CmdResult<Vec<Value>> {
        match &*self.ls.lock() {
            Ok(ls) => serde_json::from_str(ls)
                .map_err(|e| relay::failed(format!("ccrpc ls 的输出读不出：{e}"))),
            Err(Some(why)) => Err(relay::failed(format!("ccrpc 不可用：{why}"))),
            Err(None) => Err(relay::invalid("还没有拿到 ccrpc ls 的列表")),
        }
    }

    /// A new keep loop takes over the watch; returns its generation.
    pub(crate) fn begin_keep(&self) -> u64 {
        let mut keep = self.keep.lock();
        keep.generation += 1;
        keep.on = true;
        keep.generation
    }

    fn is_current(&self, keep: &Keep, generation: u64) -> bool {
        keep.on && keep.generation == generation && !self.stopped()
    }

    /// The keep loop `generation` is still the one that should run.
    pub(crate) fn keep_current(&self, generation: u64) -> bool {
        self.is_current(&self.keep.lock(), generation)
    }

    /// Runs `start` unless the keep loop `generation` was ended meanwhile.
    pub(crate) fn start_if_current(
        &self,
        generation: u64,
        start: impl FnOnce() -> relay::R<()>,
    ) -> relay::R<()> {
        let keep = self.keep.lock();
        if self.is_current(&keep, generation) {
            start()?;
        }
        Ok(())
    }

    /// The front end asks for the watch (`on`) or not. Without a keep loop `run` does that; with one its watch stays
    /// (a second one would stop it), and this returns the list to send the front end at once, if there is one yet.
    pub(crate) fn front_end_watch(
        &self,
        on: bool,
        run: impl FnOnce() -> relay::R<()>,
    ) -> relay::R<Option<String>> {
        let mut keep = self.keep.lock();
        keep.wanted = on;
        if !keep.on || self.stopped() {
            run()?;
            return Ok(None);
        }
        if !on {
            return Ok(None);
        }
        match &*self.ls.lock() {
            Ok(ls) => Ok(Some(ls.clone())),
            Err(Some(why)) => Err(relay::failed(format!("ccrpc 不可用：{why}"))),
            Err(None) => Ok(None),
        }
    }

    /// Ends the keep loop: `stop_watch` stops its watch, then `resume` runs if the front end still wants one. The
    /// cached list goes with it: nothing would keep it up to date.
    pub(crate) fn end_keep(&self, stop_watch: impl FnOnce(), resume: impl FnOnce()) {
        let mut keep = self.keep.lock();
        keep.on = false;
        keep.generation += 1;
        stop_watch();
        *self.ls.lock() = Err(None);
        if keep.wanted {
            resume();
        }
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// The Connect methods only remote clients call (`remote`, outside the method table, so the desktop app's commands
/// and bindings stay as they are). `None`: not one of them.
pub(crate) async fn dispatch(host: &Host, method: &str, args: Value) -> Option<CmdResult<Value>> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Shared {
        holder_id: String,
    }
    #[derive(serde::Deserialize)]
    struct Command {
        conn: String,
        command: Value,
    }
    if !matches!(
        method,
        "connect_states" | "ccrpc_ls" | "connect_shared" | "connect_command"
    ) {
        return None;
    }
    let Some(hub) = host.connect.clone() else {
        return Some(Err(AppError {
            code: ErrorCode::InvalidQuery,
            message: "这个 Host 不提供 Connect 状态".into(),
        }));
    };
    let parse = |e: serde_json::Error| AppError {
        code: ErrorCode::InvalidQuery,
        message: format!("参数不对：{e}"),
    };
    Some(match method {
        "connect_states" => serde_json::to_value(hub.states()).map_err(internal),
        "ccrpc_ls" => hub.ls_entries().map(Value::from),
        "connect_shared" => match serde_json::from_value::<Shared>(args) {
            Ok(a) => host
                .connect_shared(&hub, a.holder_id)
                .await
                .map(Value::from),
            Err(e) => Err(parse(e)),
        },
        _ => match serde_json::from_value::<Command>(args) {
            Ok(a) => hub.command(&a.conn, a.command).await,
            Err(e) => Err(parse(e)),
        },
    })
}

impl Host {
    /// The Host's connection to `holder_id`: the live one it has, or a new one.
    pub(crate) async fn connect_shared(
        &self,
        hub: &Arc<ConnectHub>,
        holder_id: String,
    ) -> CmdResult<String> {
        let _one = hub.shared.lock().await;
        if let Some(conn) = hub.live_conn_for(&holder_id) {
            return Ok(conn);
        }
        // claude was started again in this Pane, or the last connection is being disconnected: a ccrpc still attached
        // must go first, a second attach is refused.
        let ended = hub.ended_conns_for(&holder_id);
        for conn in &ended {
            // Replaced, which a client's window must not take for a failure.
            (self.events)(HostEvent::CcrpcClosing { conn: conn.clone() });
            // An error here means it already exited.
            let _ = self.ccrpc_disconnect(conn.clone()).await;
        }
        let mut going = ended;
        going.extend(hub.disconnecting_conns_for(&holder_id));
        if !going.is_empty() {
            let hub = hub.clone();
            tokio::task::spawn_blocking(move || hub.wait_gone(&going, ENDED_CONN_WAIT))
                .await
                .map_err(internal)?;
        }
        self.attach(holder_id).await
    }

    /// The Host's connections as it folds them, as JSON: what a window that lost its own state starts from.
    pub async fn connect_snapshot(&self) -> CmdResult<String> {
        let states = self
            .connect
            .as_ref()
            .map(|hub| hub.states())
            .unwrap_or_default();
        serde_json::to_string(&states).map_err(internal)
    }

    /// One call from a client on the LAN (`remote` methods only). The desktop's front end learns from these about
    /// connections it did not start itself; its own calls never come through here.
    pub async fn remote_call(&self, method: &str, args: Value) -> CmdResult<Value> {
        if method == "ccrpc_disconnect"
            && let Some(conn) = args["conn"].as_str()
        {
            (self.events)(HostEvent::CcrpcClosing {
                conn: conn.to_string(),
            });
        }
        let holder_id = (method == "connect_shared")
            .then(|| args["holderId"].as_str().map(str::to_string))
            .flatten();
        let out = crate::dispatch(self, method, args, true).await;
        if matches!(
            method,
            "ccrpc_new" | "ccrpc_resume" | "ccrpc_fork" | "connect_shared"
        ) && let Ok(Value::String(conn)) = &out
        {
            (self.events)(HostEvent::CcrpcRemoteConn {
                conn: conn.clone(),
                holder_id,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    struct Fixture {
        hub: Arc<ConnectHub>,
        sink: Sink,
        events: mpsc::Receiver<HostEvent>,
        written: mpsc::Receiver<(String, Value)>,
    }

    fn fixture() -> Fixture {
        let (etx, events) = mpsc::channel();
        let etx = Mutex::new(etx);
        let (wtx, written) = mpsc::channel();
        let wtx = Mutex::new(wtx);
        let hub = ConnectHub::with_throttle(
            Arc::new(move |e| {
                let _ = etx.lock().send(e);
            }),
            Arc::new(move |conn, line| {
                let _ = wtx
                    .lock()
                    .send((conn.to_string(), serde_json::from_str(line).unwrap()));
                Ok(())
            }),
            Duration::ZERO,
        );
        let sink = hub.sink();
        Fixture {
            hub,
            sink,
            events,
            written,
        }
    }

    impl Fixture {
        fn line(&self, v: Value) {
            (self.sink)(Out::Line {
                conn: "c1".into(),
                line: v.to_string(),
            });
        }

        fn drain(&self) -> Vec<HostEvent> {
            self.events.try_iter().collect()
        }

        fn wrote(&self) -> (String, Value) {
            self.written.recv_timeout(Duration::from_secs(5)).unwrap()
        }

        fn last_state(&self) -> ConnState {
            self.drain()
                .into_iter()
                .filter_map(|e| match e {
                    HostEvent::ConnectState(s) => Some(*s),
                    _ => None,
                })
                .next_back()
                .expect("a ConnectState event")
        }
    }

    #[test]
    fn folds_a_turn_and_keeps_its_own_responses() {
        let f = fixture();
        f.hub.register("c1", Some("H1".into()));
        assert_eq!(f.last_state().holder_id.as_deref(), Some("H1"));

        f.line(json!({ "type": "ready", "protocol": 2, "holder_id": "H1", "session_id": "s1", "cwd": "/w", "claude_version": "2.1", "state": "idle", "size": [80, 24], "warnings": [] }));
        let s = f.last_state();
        assert!(s.ready);
        assert_eq!(s.session_id.as_deref(), Some("s1"));
        // The Host asks for the background tasks itself after `ready`.
        let (conn, cmd) = f.wrote();
        assert_eq!(conn, "c1");
        assert_eq!(cmd["type"], "get_tasks");
        let id = cmd["id"].as_str().unwrap().to_string();
        assert!(id.starts_with('h'));
        let tasks = json!([{ "id": "b1", "kind": "shell", "tool_use_id": null, "description": "sleep", "status": "running" }]);
        f.line(json!({ "type": "response", "id": id, "command": "get_tasks", "success": true, "data": { "tasks": tasks } }));
        let events = f.drain();
        // Its response is the Host's: not relayed, folded into the tasks.
        assert!(
            !events.iter().any(
                |e| matches!(e, HostEvent::CcrpcLine { line, .. } if line.contains("get_tasks"))
            )
        );
        let s = events
            .into_iter()
            .filter_map(|e| match e {
                HostEvent::ConnectState(s) => Some(*s),
                _ => None,
            })
            .next_back()
            .unwrap();
        assert_eq!(s.tasks, tasks.as_array().unwrap().clone());

        f.line(json!({ "type": "state", "state": "busy" }));
        assert_eq!(f.last_state().state, "busy");
        f.line(json!({ "type": "permission_request", "request_id": "t1", "tool_name": "Bash", "input": {}, "agent_id": null, "options": null, "layout_source": null }));
        f.line(json!({ "type": "state", "state": "waiting", "waiting": { "kind": "permission", "waiting_for": "permission prompt" } }));
        let s = f.last_state();
        assert_eq!(s.state, "waiting");
        assert_eq!(s.requests.len(), 1);
        f.line(json!({ "type": "state", "state": "busy" }));
        f.line(json!({ "type": "block", "kind": "tool_result", "tool_use_id": "t1" }));
        assert!(f.last_state().requests.is_empty());
        // Other lines are relayed as before.
        f.line(json!({ "type": "block", "kind": "text", "text": "hi" }));
        assert!(
            f.drain()
                .iter()
                .any(|e| matches!(e, HostEvent::CcrpcLine { line, .. } if line.contains("\"hi\"")))
        );
        f.line(json!({ "type": "turn_end", "prompt_id": "p", "aborted": false }));
        let s = f.last_state();
        assert_eq!(s.last_turn.unwrap()["prompt_id"], "p");
        assert_eq!(f.wrote().1["type"], "get_tasks");
        // A response to someone else's id (the desktop app's `g<n>`) is relayed.
        f.line(json!({ "type": "response", "id": "g1", "command": "abort", "success": true }));
        assert!(
            f.drain()
                .iter()
                .any(|e| matches!(e, HostEvent::CcrpcLine { .. }))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_command_gets_its_response_and_an_exit_answers_the_rest() {
        let f = fixture();
        f.hub.register("c1", Some("H1".into()));
        f.line(json!({ "type": "permission_request", "request_id": "t1", "tool_name": "Bash", "input": {}, "agent_id": "a1", "options": null, "layout_source": null }));
        assert_eq!(f.last_state().requests.len(), 1);

        let hub = f.hub.clone();
        let call = tokio::spawn(async move {
            hub.command(
                "c1",
                json!({ "type": "answer_permission", "request_id": "t1", "decision": "allow" }),
            )
            .await
        });
        // Blocks one worker of the multi-threaded runtime; the command runs on another.
        let (_, cmd) = f.wrote();
        assert_eq!(cmd["type"], "answer_permission");
        assert_eq!(cmd["decision"], "allow");
        let id = cmd["id"].as_str().unwrap();
        assert!(id.starts_with('h'));
        let resp = json!({ "type": "response", "id": id, "command": "answer_permission", "success": true });
        (f.sink)(Out::Line {
            conn: "c1".into(),
            line: resp.to_string(),
        });
        assert_eq!(call.await.unwrap().unwrap(), resp);
        // Answered: the card is gone.
        assert!(f.hub.states()[0].requests.is_empty());

        // Unknown connection.
        assert!(
            f.hub
                .command("c9", json!({ "type": "abort" }))
                .await
                .is_err()
        );

        // A command still waiting when ccrpc exits.
        let hub = f.hub.clone();
        let call = tokio::spawn(async move { hub.command("c1", json!({ "type": "abort" })).await });
        let deadline = Instant::now() + Duration::from_secs(5);
        while f.hub.inner.lock().pending.is_empty() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        (f.sink)(Out::Exit(relay::ExitEvent {
            conn: "c1".into(),
            code: Some(1),
            stderr: "boom".into(),
        }));
        let r = call.await.unwrap().unwrap();
        assert_eq!(r["success"], false);
        assert_eq!(r["error"]["code"], "exited");
        let events = f.drain();
        let names: Vec<_> = events.iter().map(HostEvent::name).collect();
        let n = names.len();
        assert_eq!(
            names[n - 3..],
            [
                "connect-state-event",
                "connect-removed-event",
                "ccrpc-exit-event"
            ]
        );
        assert_eq!(events[n - 3].payload()["procExit"]["stderr"], "boom");
        assert_eq!(events[n - 2].payload(), json!({ "conn": "c1" }));
        assert!(f.hub.states().is_empty());
        // Late lines do not bring it back.
        f.line(json!({ "type": "state", "state": "idle" }));
        f.hub.register("c1", None);
        assert!(f.hub.states().is_empty());
    }

    #[test]
    fn throttles_a_connections_state_and_sends_the_last() {
        let (etx, events) = mpsc::channel();
        let etx = Mutex::new(etx);
        let hub = ConnectHub::with_throttle(
            Arc::new(move |e| {
                let _ = etx.lock().send(e);
            }),
            Arc::new(|_, _| Ok(())),
            Duration::from_millis(100),
        );
        let sink = hub.sink();
        for s in ["busy", "idle", "busy", "idle"] {
            sink(Out::Line {
                conn: "c1".into(),
                line: json!({ "type": "state", "state": s }).to_string(),
            });
        }
        let mut states = Vec::new();
        while let Ok(e) = events.recv_timeout(Duration::from_millis(400)) {
            if let HostEvent::ConnectState(s) = e {
                states.push(s.state);
            }
        }
        // The first at once, the rest as one once the throttle allows: the last.
        assert_eq!(states, ["busy", "idle"]);
    }

    #[test]
    fn an_exit_before_any_line_or_registration_is_still_said() {
        let f = fixture();
        (f.sink)(Out::Exit(relay::ExitEvent {
            conn: "c3".into(),
            code: Some(2),
            stderr: "ccrpc: 找不到 Holder".into(),
        }));
        let events = f.drain();
        let names: Vec<_> = events.iter().map(HostEvent::name).collect();
        assert_eq!(
            names,
            [
                "connect-state-event",
                "connect-removed-event",
                "ccrpc-exit-event"
            ]
        );
        let p = events[0].payload();
        assert_eq!(p["conn"], "c3");
        assert_eq!(
            p["procExit"],
            json!({ "code": 2, "stderr": "ccrpc: 找不到 Holder" })
        );
        // The registration after the spawn returned comes too late: nothing comes back.
        f.hub.register("c3", Some("H1".into()));
        assert!(f.hub.states().is_empty());
        assert!(f.drain().is_empty());
    }

    #[test]
    fn a_disconnect_is_in_the_final_state() {
        let f = fixture();
        f.hub.register("c1", Some("H1".into()));
        f.hub.mark_disconnected("c1");
        assert!(f.last_state().disconnected);
        // Unknown connections are left alone.
        f.hub.mark_disconnected("c9");
        assert_eq!(f.hub.states().len(), 1);
        (f.sink)(Out::Exit(relay::ExitEvent {
            conn: "c1".into(),
            code: Some(0),
            stderr: String::new(),
        }));
        let s = f.last_state();
        assert!(s.disconnected && s.proc_exit.is_some());
    }

    #[test]
    fn responses_to_ids_the_host_did_not_send_are_relayed() {
        let f = fixture();
        f.hub.register("c1", None);
        f.drain();
        f.line(json!({ "type": "response", "id": "h77", "command": "abort", "success": true }));
        assert!(matches!(&f.drain()[..], [HostEvent::CcrpcLine { .. }]));
    }

    #[test]
    fn a_holder_gets_its_newest_live_connection_and_never_an_ended_one() {
        let f = fixture();
        for c in ["c9", "c10"] {
            f.hub.register(c, Some("H1".into()));
        }
        f.hub.register("c11", Some("H2".into()));
        assert_eq!(f.hub.live_conn_for("H1").as_deref(), Some("c10"));
        let end = |conn: &str| {
            (f.sink)(Out::Line {
                conn: conn.into(),
                line: json!({ "type": "session_exit" }).to_string(),
            })
        };
        // c10's Session ended: c9 is still live.
        end("c10");
        assert_eq!(f.hub.live_conn_for("H1").as_deref(), Some("c9"));
        assert_eq!(f.hub.ended_conns_for("H1"), ["c10"]);
        // Nothing live is left to hand back, only connections to end first.
        end("c9");
        assert_eq!(f.hub.live_conn_for("H1"), None);
        let mut ended = f.hub.ended_conns_for("H1");
        ended.sort();
        assert_eq!(ended, ["c10", "c9"]);
        assert_eq!(f.hub.live_conn_for("H3"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_clients_exit_is_announced_as_deliberate() {
        let f = fixture();
        f.hub.register("c1", Some("H1".into()));
        let hub = f.hub.clone();
        let call = tokio::spawn(async move { hub.command("c1", json!({ "type": "exit" })).await });
        let id = f.wrote().1["id"].clone();
        f.line(json!({ "type": "response", "id": id, "command": "exit", "success": true }));
        call.await.unwrap().unwrap();
        assert!(
            f.drain()
                .iter()
                .any(|e| matches!(e, HostEvent::CcrpcClosing { conn } if conn == "c1"))
        );
    }

    #[test]
    fn a_failed_fork_is_closing() {
        let f = fixture();
        f.hub.register("c1", None);
        let closing = |f: &Fixture| {
            f.drain()
                .iter()
                .any(|e| matches!(e, HostEvent::CcrpcClosing { .. }))
        };
        f.line(json!({ "type": "fork", "session_id": "s2" }));
        assert!(!closing(&f));
        f.line(json!({ "type": "fork", "error": { "code": "x", "message": "m" } }));
        assert!(closing(&f));
    }

    #[test]
    fn ls_says_why_there_is_no_list() {
        let f = fixture();
        assert!(f.hub.ls_entries().unwrap_err().message.contains("还没有"));
        f.hub.set_ls(Err("找不到 toh".into()));
        assert_eq!(
            f.hub.ls_entries().unwrap_err().message,
            "ccrpc 不可用：找不到 toh"
        );
        f.hub
            .set_ls(Ok(r#"[{"holder_id":"H1","plugin":true}]"#.into()));
        assert_eq!(f.hub.ls_entries().unwrap()[0]["holder_id"], "H1");
    }
}
