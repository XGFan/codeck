//! 缓存保活（「设置 › 通用 › 缓存保活」）：Claude Code 的提示词缓存一小时不用就过期，过期后下一条提示词要把整个
//! 上下文重新写入缓存。Live Session 空闲到设定的时长（默认 40 分钟）而缓存还没过期时，Host 经 ccrpc 发一条
//! `/btw`（旁问：答案只在面板里，不进 transcript，不加长对话），读到答案后关掉面板，缓存就从这时起再算一小时。
//!
//! - 缓存上次被用到的时刻：transcript 的修改时间与上次保活的较晚者。已经过了一小时的不再续（缓存已经没了，续只是
//!   重新写入），直到它再有动静。
//! - 只续 clay 里装了插件、此刻空闲的 Session：已有连接的经那个连接发，没有的临时 `ccrpc attach`，发完就断开。
//! - 期间前端收到 `KeepAlive` 事件：面板开着时 Live 状态是 waiting，这不算等待作答。

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime};

use codeck_core::model::LiveStatus;
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::Host;
use crate::ccrpc::relay::{self, App};
use crate::connect::ConnectHub;
use crate::events::HostEvent;
use crate::settings;

/// What the `/btw` asks; its question marks it as the Host's own (`is_marker`), which no one is shown.
pub const QUESTION: &str = "ping, just say pong.";
/// What `is_marker` looks for: the question as ccrpc reads it off the screen, its last character aside.
const MARKER: &str = "ping, just say pong";
/// How long Claude Code keeps a prompt cache that is not used (it writes the one-hour kind).
pub const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const TICK: Duration = Duration::from_secs(30);
/// For each step of a refresh: ccrpc's `ready`, the panel, the answer, closing it.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);

/// A `btw` event (or request) of the Host's own `/btw`.
pub fn is_marker(m: &Value) -> bool {
    m["question"]
        .as_str()
        .is_some_and(|q| q.trim().starts_with(MARKER))
}

/// A Live Session as the keep-alive sees it.
pub(crate) struct Seen {
    pub session_id: String,
    pub idle: bool,
    /// When its transcript was last written; None when unknown (not indexed yet).
    pub written: Option<SystemTime>,
}

/// The Sessions whose cache to refresh at `now`: idle, last used at least `interval` and less than [`CACHE_TTL`]
/// ago (the later of its transcript's last write and the last refresh).
pub(crate) fn due(
    now: SystemTime,
    interval: Duration,
    seen: &[Seen],
    refreshed: &HashMap<String, SystemTime>,
) -> Vec<String> {
    seen.iter()
        .filter(|s| s.idle)
        .filter_map(|s| {
            let last = match (s.written, refreshed.get(&s.session_id)) {
                (Some(w), Some(r)) => w.max(*r),
                (Some(w), None) => w,
                (None, Some(r)) => *r,
                (None, None) => return None,
            };
            let age = now.duration_since(last).unwrap_or_default();
            (age >= interval && age < CACHE_TTL).then(|| s.session_id.clone())
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct KeepAliveState {
    /// When the Host last tried to refresh each Session (tried: a failure is not retried before the next interval).
    refreshed: Mutex<HashMap<String, SystemTime>>,
    running: Mutex<HashSet<String>>,
}

/// Checks the Live Sessions every [`TICK`] while the Host is alive.
pub(crate) fn spawn(host: Weak<Host>) {
    let _ = std::thread::Builder::new()
        .name("cv-keepalive".into())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
                let Some(host) = host.upgrade() else { break };
                host.keep_alive_tick();
            }
        });
}

/// How a Session is reached: its connection, or a Pane to attach to for the moment.
enum Route {
    Conn(Arc<ConnectHub>, String),
    Attach(String),
}

impl Host {
    fn keep_alive_tick(self: &Arc<Self>) {
        let minutes = settings::load().keep_alive_minutes();
        if minutes == 0 {
            return;
        }
        let engine = self.state.engine();
        let Ok(live) = engine.live_now() else { return };
        let seen: Vec<Seen> = live
            .iter()
            .map(|e| Seen {
                session_id: e.session_id.clone(),
                idle: e.state.status == LiveStatus::Idle,
                written: engine
                    .session_file_path(&e.session_id, None)
                    .ok()
                    .and_then(|p| std::fs::metadata(p).ok()?.modified().ok()),
            })
            .collect();
        let now = SystemTime::now();
        let ka = &self.keep_alive;
        let due = due(
            now,
            Duration::from_secs(u64::from(minutes) * 60),
            &seen,
            &ka.refreshed.lock(),
        );
        for id in due {
            if !ka.running.lock().insert(id.clone()) {
                continue;
            }
            ka.refreshed.lock().insert(id.clone(), now);
            let host = self.clone();
            let _ = std::thread::Builder::new()
                .name("cv-keepalive-ping".into())
                .spawn(move || {
                    host.keep_alive(&id);
                    host.keep_alive.running.lock().remove(&id);
                });
        }
        // Sessions no longer Live are forgotten.
        let ids: HashSet<&str> = seen.iter().map(|s| s.session_id.as_str()).collect();
        ka.refreshed
            .lock()
            .retain(|id, _| ids.contains(id.as_str()));
    }

    /// Refreshes one Session's cache, if ccrpc can reach it and it is still idle.
    fn keep_alive(&self, session_id: &str) {
        let Ok(app) = self.ccrpc.app() else { return };
        let route = match self.keep_alive_route(app, session_id) {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => {
                app.gui_line(&format!("keepalive {session_id}: {}", e.message));
                return;
            }
        };
        let emit = |active: bool, error: Option<String>| {
            (self.events)(HostEvent::KeepAlive {
                session_id: session_id.to_string(),
                active,
                error,
            })
        };
        emit(true, None);
        let (how, result) = match route {
            Route::Conn(hub, conn) => (format!("via {conn}"), via_conn(&hub, &conn)),
            Route::Attach(holder) => (format!("attach {holder}"), via_attach(app, &holder)),
        };
        let error = result.err().map(|e| e.message);
        app.gui_line(&format!(
            "keepalive {session_id} {how}: {}",
            error.as_deref().unwrap_or("ok")
        ));
        emit(false, error);
    }

    /// Where to send the `/btw`: None when the Session is not one ccrpc can drive now (said in gui.log).
    fn keep_alive_route(&self, app: &App, session_id: &str) -> relay::R<Option<Route>> {
        let ls = app.ls_once()?;
        let holder = match ls.iter().find(|e| e["session_id"] == session_id) {
            Some(e) => drivable(e),
            None => Err("不在 clay 里".to_string()),
        };
        let holder = match holder {
            Ok(h) => h,
            Err(why) => {
                app.gui_line(&format!("keepalive {session_id}: 跳过，{why}"));
                return Ok(None);
            }
        };
        if let Some(hub) = &self.connect
            && let Some(conn) = hub.live_conn_for(holder)
        {
            return Ok(Some(Route::Conn(hub.clone(), conn)));
        }
        Ok(Some(Route::Attach(holder.to_string())))
    }
}

/// The Holder to send the `/btw` to, from the Session's row of `ccrpc ls --json`; Err says why not now.
fn drivable(e: &Value) -> Result<&str, String> {
    let holder = e["holder_id"]
        .as_str()
        .ok_or_else(|| "ccrpc ls 没有给出 Holder ID".to_string())?;
    if e["plugin"] != true {
        return Err("没有 ccrpc 插件".into());
    }
    // `status` is the process file's: read as Live status is, so `shell` (a background task running) is idle.
    let status = e["status"].as_str();
    if LiveStatus::from_raw(status) != LiveStatus::Idle {
        return Err(format!("状态是 {}", status.unwrap_or("-")));
    }
    Ok(holder)
}

fn prompt() -> Value {
    json!({ "type": "prompt", "text": format!("/btw {QUESTION}") })
}

/// The response to `prompt()`: the panel is up.
fn check_btw(r: &Value) -> relay::R<()> {
    if r["success"] == true && r["data"]["delivered_as"] == "btw" {
        return Ok(());
    }
    let why = r["error"]["message"]
        .as_str()
        .unwrap_or("没有出现 /btw 面板");
    Err(relay::failed(why.to_string()))
}

/// Waits for the answer of our `/btw` among `lines` (it has used the cache by then).
fn wait_answer(lines: &Receiver<Value>) -> relay::R<()> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(m) if m["type"] == "btw" && is_marker(&m) && m["text"].is_string() => return Ok(()),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return Err(relay::io("等不到 /btw 的回答")),
            Err(RecvTimeoutError::Disconnected) => return Err(relay::io("ccrpc 已退出")),
        }
    }
}

/// Over the Session's connection, as the Host's own commands (their responses go to no client).
fn via_conn(hub: &Arc<ConnectHub>, conn: &str) -> relay::R<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| relay::io(format!("建不了运行时：{e}")))?;
    let lines = hub.observe(conn);
    check_btw(&rt.block_on(hub.command(conn, prompt()))?)?;
    let answered = wait_answer(&lines);
    // Closed even without an answer: the panel would hide the input box.
    let closed = rt.block_on(hub.command(conn, json!({ "type": "close_btw" })));
    answered?;
    closed.map(|_| ())
}

/// A ccrpc of its own for the moment (`attach`), closed when done.
fn via_attach(app: &App, holder: &str) -> relay::R<()> {
    let mut cmd = app.command();
    cmd.args(["attach", holder])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child =
        relay::spawn(&mut cmd).map_err(|e| relay::io(format!("运行不了 ccrpc attach：{e}")))?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let (tx, lines) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let Some(out) = stdout else { return };
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let mut send = |v: Value| -> relay::R<()> {
        let w = stdin
            .as_mut()
            .ok_or_else(|| relay::io("ccrpc 的 stdin 已关闭"))?;
        writeln!(w, "{v}")
            .and_then(|()| w.flush())
            .map_err(|e| relay::io(format!("写不进 ccrpc：{e}")))
    };
    let result = (|| {
        wait_for(&lines, |m| m["type"] == "ready", "ccrpc 没有就绪")?;
        let mut p = prompt();
        p["id"] = "k1".into();
        send(p)?;
        let r = wait_for(
            &lines,
            |m| m["type"] == "response" && m["id"] == "k1",
            "ccrpc 没有回应 /btw",
        )?;
        check_btw(&r)?;
        let answered = wait_answer(&lines);
        send(json!({ "id": "k2", "type": "close_btw" }))?;
        wait_for(
            &lines,
            |m| m["type"] == "response" && m["id"] == "k2",
            "ccrpc 没有关掉 /btw 面板",
        )?;
        answered
    })();
    // Closing stdin detaches ccrpc; the Session keeps running.
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn wait_for(lines: &Receiver<Value>, want: impl Fn(&Value) -> bool, why: &str) -> relay::R<Value> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(m) if want(&m) => return Ok(m),
            Ok(_) => {}
            Err(_) => return Err(relay::io(why.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn seen(id: &str, idle: bool, written: Option<SystemTime>) -> Seen {
        Seen {
            session_id: id.into(),
            idle,
            written,
        }
    }

    #[test]
    fn due_from_the_interval_until_the_cache_is_gone() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let s = [seen("a", true, Some(t0))];
        let none = HashMap::new();
        assert!(due(t0 + 39 * MIN, 40 * MIN, &s, &none).is_empty());
        assert_eq!(due(t0 + 40 * MIN, 40 * MIN, &s, &none), ["a"]);
        // An hour on, the cache is already gone: refreshing would only write it again.
        assert!(due(t0 + 61 * MIN, 40 * MIN, &s, &none).is_empty());
    }

    #[test]
    fn a_refresh_starts_the_interval_again_and_busy_or_unknown_sessions_wait() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let refreshed = HashMap::from([("a".to_string(), t0 + 40 * MIN)]);
        let s = [seen("a", true, Some(t0))];
        assert!(due(t0 + 70 * MIN, 40 * MIN, &s, &refreshed).is_empty());
        assert_eq!(due(t0 + 80 * MIN, 40 * MIN, &s, &refreshed), ["a"]);
        let busy = [seen("a", false, Some(t0))];
        assert!(due(t0 + 45 * MIN, 40 * MIN, &busy, &HashMap::new()).is_empty());
        let unknown = [seen("a", true, None)];
        assert!(due(t0 + 45 * MIN, 40 * MIN, &unknown, &HashMap::new()).is_empty());
    }

    #[test]
    fn a_session_running_a_background_task_is_idle_enough() {
        let row = |status: &str, plugin: bool| json!({ "holder_id": "m1", "session_id": "a", "status": status, "plugin": plugin });
        // The process file says `shell` while a background shell or Monitor runs; the input box is free.
        assert_eq!(drivable(&row("shell", true)), Ok("m1"));
        assert_eq!(drivable(&row("idle", true)), Ok("m1"));
        assert_eq!(drivable(&row("busy", true)), Err("状态是 busy".into()));
        assert!(drivable(&row("waiting", true)).is_err());
        assert_eq!(drivable(&row("idle", false)), Err("没有 ccrpc 插件".into()));
    }

    #[test]
    fn only_the_hosts_own_btw_is_a_marker() {
        assert!(is_marker(&json!({ "type": "btw", "question": QUESTION })));
        assert!(!is_marker(
            &json!({ "type": "btw", "question": "这段代码为什么慢" })
        ));
    }
}
