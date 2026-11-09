//! Connect state held by the Host (ADR-0011; only with `HostConfig::track_connect`, i.e. the daemon): every
//! connection's ccrpc lines are folded into a [`ConnState`] that all of the Host's clients share, and clients
//! send ccrpc commands through the Host ([`ConnectHub::command`]), which gives them ids `h<n>` and hands back
//! the response. The desktop app keeps doing this in its own store (`src/features/connect`).

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
    stopped: AtomicBool,
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

    /// False when the line is the Host's own (a response to an `h<n>` command): not relayed.
    fn on_line(self: &Arc<Self>, conn: &str, line: &str) -> bool {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return true;
        };
        if v["type"] == "response" {
            if let Some(id) = v["id"].as_str().filter(|id| id.starts_with('h')) {
                self.on_response(id, v.clone());
                return false;
            }
            return true;
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

    fn on_response(self: &Arc<Self>, id: &str, r: Value) {
        let mut inner = self.inner.lock();
        let Some(p) = inner.pending.remove(id) else {
            return;
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
        if let Some(w) = p.waiter {
            let _ = w.send(r);
        }
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
        if let Some(mut e) = inner.conns.remove(conn) {
            e.state.proc_exit(code, stderr);
            (self.events)(HostEvent::ConnectState(Box::new(e.state)));
            (self.events)(HostEvent::ConnectRemoved {
                conn: conn.to_string(),
            });
        }
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

    /// The live connection to `holder_id`, if any.
    fn conn_for(&self, holder_id: &str) -> Option<String> {
        let inner = self.inner.lock();
        let mut c: Vec<_> = inner
            .conns
            .values()
            .filter(|e| e.state.holder_id.as_deref() == Some(holder_id))
            .map(|e| e.state.conn.clone())
            .collect();
        c.sort();
        c.pop()
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

    /// The latest `ccrpc ls --json` list, as ccrpc printed it.
    pub fn latest_ls(&self) -> Option<String> {
        self.ls.lock().as_ref().ok().cloned()
    }

    fn ls_entries(&self) -> CmdResult<Vec<Value>> {
        match &*self.ls.lock() {
            Ok(ls) => serde_json::from_str(ls)
                .map_err(|e| relay::failed(format!("ccrpc ls 的输出读不出：{e}"))),
            Err(Some(why)) => Err(relay::failed(format!("ccrpc 不可用：{why}"))),
            Err(None) => Err(relay::invalid("还没有拿到 ccrpc ls 的列表")),
        }
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// The Connect methods only the daemon serves (`remote`, outside the method table, so the desktop app's
/// commands and bindings stay as they are). `None`: not one of them.
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
    /// The Host's connection to `holder_id`: the one it has, or a new one.
    async fn connect_shared(&self, hub: &ConnectHub, holder_id: String) -> CmdResult<String> {
        let _one = hub.shared.lock().await;
        if let Some(conn) = hub.conn_for(&holder_id) {
            return Ok(conn);
        }
        self.ccrpc_connect(holder_id).await
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
