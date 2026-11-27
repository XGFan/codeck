//! 「已完成」与通知（ADR-0019）：Host 按每个 Live Session 的状态与各端「正在看」决定发不发、发到哪一端、何时删，规则在
//! [`rules`]。Mac 的通知经桌面外壳注入的 [`Notifier`]，iPhone 的经 Bark（[`bark`]）。
//!
//! - 状态：Live（进程文件，worker 的 `LiveChanged`）；已连接的取 ConnState 的状态，有请求卡片的算等待作答；Host 在续缓存的
//!   Session（`keepalive.rs`，`/btw` 面板开着的几秒）保持原来的状态。
//! - 正在看：各端经 `set_viewing` 报来，按调用方记（[`Viewer`]）；局域网设备的事件连接全断了就清掉它的。
//! - 每秒推进一次，状态或「正在看」一变就立即推进。

pub mod bark;
pub mod rules;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use codeck_core::model::{LiveChanged, LiveStatus};
use parking_lot::Mutex;
use serde::Serialize;

use self::rules::{Action, Channels, Delays, Due, Ms, Status, Subject, Target};
use crate::Host;
use crate::commands::{CmdResult, internal};
use crate::connect::ConnState;
use crate::events::HostEvent;
use crate::settings::{self, NotifyFile, NotifySettings};
use codeck_core::model::{AppError, ErrorCode};

const TICK: Duration = Duration::from_secs(1);
const MIN: Ms = 60_000;

/// A notification for the Mac's Notification Center.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// Names it: a later one with the same `id` takes its place, and `Notifier::remove` finds it by this.
    pub id: String,
    pub title: String,
    pub body: String,
    /// Opened when it is clicked (a `codeck://` link).
    pub url: String,
    /// Notification Center keeps the notifications of one thread together.
    pub thread: Option<String>,
    /// Time-sensitive: shown during a Focus that lets such notifications through.
    pub time_sensitive: bool,
}

/// The Mac's notifications, as the desktop app's shell provides them.
pub trait Notifier: Send + Sync {
    /// Someone uses the Mac now (keyboard or mouse lately, the screen not locked).
    fn present(&self) -> bool;
    /// Shows `notice`, in place of the one with its `id` when that is still there.
    fn send(&self, notice: Notice);
    /// Takes the notifications with these `id`s out of Notification Center.
    fn remove(&self, ids: Vec<String>);
}

/// Who calls the Host: what `set_viewing` keeps apart.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Viewer {
    /// The desktop app's window (in-process; no `CALLER` set).
    Desktop,
    /// A paired LAN device, by its id.
    Device(String),
    /// A client on a loopback listener (development, E2E).
    Loopback,
}

tokio::task_local! {
    /// The caller of the method being run, set around LAN RPCs.
    pub static CALLER: Viewer;
}

/// The last push to the iPhone that failed.
#[derive(Serialize, specta::Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PushError {
    pub message: String,
    /// Epoch milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub at: f64,
}

/// 「设置 › 通用」 的通知 and how the last push went.
#[derive(Serialize, specta::Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotifyInfo {
    pub settings: NotifySettings,
    /// The last push failed, and none went out since.
    pub last_push_error: Option<PushError>,
}

#[derive(Default)]
struct Inner {
    state: rules::State,
    /// The Live Sessions' statuses from the latest `LiveChanged`.
    live: BTreeMap<String, LiveStatus>,
    viewing: BTreeMap<Viewer, String>,
    /// Each viewer's open event connections.
    streams: HashMap<Viewer, usize>,
    /// The 「已完成」 Sessions last reported.
    done: Vec<String>,
    push_error: Option<PushError>,
}

pub(crate) struct Attention {
    inner: Mutex<Inner>,
    notifier: Option<Arc<dyn Notifier>>,
    wake: crossbeam_channel::Sender<()>,
    woken: crossbeam_channel::Receiver<()>,
}

/// While it lives, its viewer has an event connection open; the last one to go clears what the viewer looked at.
pub(crate) struct ViewerStream {
    attention: Arc<Attention>,
    viewer: Viewer,
}

impl Drop for ViewerStream {
    fn drop(&mut self) {
        let mut inner = self.attention.inner.lock();
        let left = inner.streams.get_mut(&self.viewer).map(|n| {
            *n -= 1;
            *n
        });
        if left == Some(0) {
            inner.streams.remove(&self.viewer);
            if inner.viewing.remove(&self.viewer).is_some() {
                drop(inner);
                self.attention.poke();
            }
        }
    }
}

fn now_ms() -> Ms {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as Ms)
}

/// Each Session's status: Live from its process file, a connected one's from ccrpc; one with a request card waits.
fn statuses(live: &BTreeMap<String, LiveStatus>, conns: &[ConnState]) -> BTreeMap<String, Status> {
    let mut out: BTreeMap<String, Status> = live
        .iter()
        .map(|(id, s)| {
            let s = match s {
                LiveStatus::Busy => Status::Busy,
                LiveStatus::Waiting => Status::Waiting,
                _ => Status::Idle,
            };
            (id.clone(), s)
        })
        .collect();
    let live_conn =
        |c: &ConnState| c.proc_exit.is_none() && !c.session_exited && c.start_failed.is_none();
    for c in conns {
        let Some(id) = &c.session_id else { continue };
        if !out.contains_key(id) || !live_conn(c) {
            continue;
        }
        let s = match c.state.as_str() {
            "busy" => Status::Busy,
            "idle" => Status::Idle,
            "waiting" => Status::Waiting,
            _ => continue,
        };
        out.insert(id.clone(), s);
    }
    for c in conns {
        let id = c.session_id.as_ref().unwrap_or(&c.conn);
        if c.proc_exit.is_none() && !c.requests.is_empty() && out.contains_key(id) {
            out.insert(id.clone(), Status::Waiting);
        }
    }
    out
}

impl Attention {
    pub(crate) fn new(notifier: Option<Arc<dyn Notifier>>) -> Self {
        let (wake, woken) = crossbeam_channel::bounded(1);
        Attention {
            inner: Mutex::default(),
            notifier,
            wake,
            woken,
        }
    }

    /// What wakes the stepping thread ([`spawn`]).
    pub(crate) fn woken(&self) -> crossbeam_channel::Receiver<()> {
        self.woken.clone()
    }

    /// Steps again now rather than at the next tick.
    fn poke(&self) {
        let _ = self.wake.try_send(());
    }

    /// A `LiveChanged` went out.
    pub(crate) fn on_live(&self, l: &LiveChanged) {
        self.inner.lock().live = l
            .live
            .iter()
            .map(|e| (e.session_id.clone(), e.state.status))
            .collect();
        self.poke();
    }

    fn set_viewing(&self, viewer: Viewer, session_id: Option<String>) {
        let mut inner = self.inner.lock();
        let changed = match session_id {
            Some(id) => inner.viewing.insert(viewer, id.clone()).as_ref() != Some(&id),
            None => inner.viewing.remove(&viewer).is_some(),
        };
        drop(inner);
        if changed {
            self.poke();
        }
    }

    pub(crate) fn stream(self: &Arc<Self>, viewer: Viewer) -> ViewerStream {
        *self.inner.lock().streams.entry(viewer.clone()).or_default() += 1;
        ViewerStream {
            attention: self.clone(),
            viewer,
        }
    }

    fn pushed(&self, result: &Result<(), String>) {
        self.inner.lock().push_error = result.as_ref().err().map(|e| PushError {
            message: e.clone(),
            at: now_ms() as f64,
        });
    }
}

/// Steps every [`TICK`], or at once when poked, while the Host is alive.
pub(crate) fn spawn(host: Weak<Host>, woken: crossbeam_channel::Receiver<()>) {
    let _ = std::thread::Builder::new()
        .name("cv-attention".into())
        .spawn(move || {
            loop {
                if let Err(crossbeam_channel::RecvTimeoutError::Disconnected) =
                    woken.recv_timeout(TICK)
                {
                    break;
                }
                let Some(host) = host.upgrade() else { break };
                host.attention_tick();
            }
        });
}

/// Bark as set up: its server and device key.
struct BarkTo {
    server: String,
    device_key: String,
}

impl Host {
    /// An event connection of `viewer` is open while this lives.
    pub(crate) fn viewer_stream(&self, viewer: Viewer) -> ViewerStream {
        self.attention.stream(viewer)
    }

    fn attention_tick(&self) {
        let s = settings::load().notify.resolved();
        let minutes = |m: Option<u32>| m.map(|m| Ms::from(m) * MIN);
        let delays = Delays {
            waiting: minutes(s.waiting_minutes),
            done: minutes(s.done_minutes),
        };
        let bark = (!s.bark_device_key.is_empty()).then(|| BarkTo {
            server: s.bark_server.clone(),
            device_key: s.bark_device_key.clone(),
        });
        let channels = Channels {
            mac: self.attention.notifier.as_ref().map(|n| n.present()),
            phone: bark.is_some(),
            escalate: minutes(s.escalate_minutes),
        };
        let conns = self
            .connect
            .as_ref()
            .map(|h| h.states())
            .unwrap_or_default();
        let refreshing = self.keep_alive.refreshing();
        let at = now_ms();
        let (actions, done) = {
            let mut inner = self.attention.inner.lock();
            let mut now = statuses(&inner.live, &conns);
            for id in &refreshing {
                if let (Some(t), Some(s)) = (inner.state.tracks.get(id), now.get_mut(id)) {
                    *s = t.status;
                }
            }
            let viewing: BTreeSet<String> = inner.viewing.values().cloned().collect();
            let (state, actions) =
                rules::advance(&inner.state, &now, at, &viewing, &delays, &channels);
            inner.state = state;
            let done = inner.state.done();
            let changed = done != inner.done;
            if changed {
                inner.done = done.clone();
            }
            (actions, changed.then_some(done))
        };
        if let Some(done) = done {
            self.emit_event(HostEvent::Attention { done });
        }
        if !actions.is_empty() {
            self.perform(actions, at, bark, &conns);
        }
    }

    fn perform(&self, actions: Vec<Action>, at: Ms, bark: Option<BarkTo>, conns: &[ConnState]) {
        let mut remove_mac = Vec::new();
        for a in actions {
            match a {
                Action::Send { id, items, to } => {
                    let m = rules::message(&self.subjects(&items, conns), at);
                    let url = format!("codeck://session/{}", m.session_id);
                    match to {
                        Target::Mac => {
                            if let Some(n) = &self.attention.notifier {
                                n.send(Notice {
                                    id,
                                    title: m.title,
                                    body: m.body,
                                    url,
                                    thread: m.thread,
                                    time_sensitive: m.time_sensitive,
                                });
                            }
                        }
                        Target::Phone => {
                            let Some(b) = &bark else { continue };
                            let body = bark::push_body(
                                &b.device_key,
                                &bark::Push {
                                    id: Some(id),
                                    title: m.title,
                                    body: m.body,
                                    url: Some(url),
                                    time_sensitive: m.time_sensitive,
                                },
                            );
                            let (server, attention) = (b.server.clone(), self.attention.clone());
                            let _ = std::thread::Builder::new().name("cv-bark".into()).spawn(
                                move || {
                                    let r = bark::send(&server, &body);
                                    if let Err(e) = &r {
                                        tracing::warn!("bark push failed: {e}");
                                    }
                                    attention.pushed(&r);
                                },
                            );
                        }
                    }
                }
                Action::Remove {
                    id,
                    to: Target::Mac,
                } => remove_mac.push(id),
                Action::Remove {
                    id,
                    to: Target::Phone,
                } => {
                    let Some(b) = &bark else { continue };
                    let (server, body) = (b.server.clone(), bark::delete_body(&b.device_key, &id));
                    let _ = std::thread::Builder::new()
                        .name("cv-bark".into())
                        .spawn(move || {
                            if let Err(e) = bark::send(&server, &body) {
                                tracing::warn!("bark delete {id} failed: {e}");
                            }
                        });
                }
            }
        }
        if !remove_mac.is_empty()
            && let Some(n) = &self.attention.notifier
        {
            n.remove(remove_mac);
        }
    }

    /// What the notification says about each Session: its title and Project, as the index has them; one not indexed
    /// yet is 「新 Session」 in its connection's directory.
    fn subjects(&self, items: &[Due], conns: &[ConnState]) -> Vec<Subject> {
        let engine = self.state.engine();
        let projects = engine.list_projects().unwrap_or_default();
        items
            .iter()
            .map(|d| {
                let summary = engine.get_session(&d.id).ok().map(|s| s.summary);
                let project = summary
                    .as_ref()
                    .and_then(|s| projects.iter().find(|p| p.id == s.project_id))
                    .map(|p| p.display_name.clone())
                    .or_else(|| {
                        conns
                            .iter()
                            .find(|c| c.session_id.as_deref() == Some(&d.id))
                            .and_then(|c| c.cwd.as_deref())
                            .and_then(|cwd| cwd.rsplit('/').find(|s| !s.is_empty()))
                            .map(str::to_string)
                    });
                Subject {
                    due: d.clone(),
                    title: summary.map_or_else(|| "新 Session".to_string(), |s| s.title),
                    project,
                }
            })
            .collect()
    }

    /// Tells the Host which Session this caller looks at (`None`: none): the desktop window while in front, the
    /// iPhone's Session page while the app is.
    pub async fn set_viewing(&self, session_id: Option<String>) -> CmdResult<()> {
        let viewer = CALLER.try_with(Clone::clone).unwrap_or(Viewer::Desktop);
        self.attention.set_viewing(viewer, session_id);
        Ok(())
    }

    /// The 「已完成」 Sessions now.
    pub async fn attention_done(&self) -> CmdResult<Vec<String>> {
        Ok(self.attention.inner.lock().done.clone())
    }

    pub async fn notify_settings(&self) -> CmdResult<NotifyInfo> {
        let settings = tokio::task::spawn_blocking(|| settings::load().notify.resolved())
            .await
            .map_err(internal)?;
        Ok(NotifyInfo {
            settings,
            last_push_error: self.attention.inner.lock().push_error.clone(),
        })
    }

    pub async fn set_notify_settings(&self, settings: NotifySettings) -> CmdResult<NotifyInfo> {
        check_server(&settings.bark_server)?;
        let file = NotifyFile::from_settings(&settings);
        let saved = tokio::task::spawn_blocking(move || {
            settings::update(|s| s.notify = file).map_err(|e| AppError {
                code: ErrorCode::Internal,
                message: format!("保存设置失败：{e}"),
            })
        })
        .await
        .map_err(internal)??;
        if saved.notify.bark.device_key.is_none() {
            self.attention.inner.lock().push_error = None;
        }
        self.attention.poke();
        Ok(NotifyInfo {
            settings: saved.notify.resolved(),
            last_push_error: self.attention.inner.lock().push_error.clone(),
        })
    }

    /// Pushes a test notification with these values (not saved).
    pub async fn test_bark(&self, server: String, device_key: String) -> CmdResult<()> {
        check_server(&server)?;
        let key = device_key.trim().to_string();
        if key.is_empty() {
            return Err(AppError {
                code: ErrorCode::InvalidQuery,
                message: "没有填 Device Key".into(),
            });
        }
        let server = bark::server_or_default(Some(&server));
        let body = bark::push_body(
            &key,
            &bark::Push {
                id: None,
                title: "CoDeck".into(),
                body: "测试推送".into(),
                url: None,
                time_sensitive: false,
            },
        );
        tokio::task::spawn_blocking(move || bark::send(&server, &body))
            .await
            .map_err(internal)?
            .map_err(|message| AppError {
                code: ErrorCode::Internal,
                message,
            })
    }
}

/// A blank server is the default one; any other is an http(s) address.
fn check_server(server: &str) -> CmdResult<()> {
    let s = server.trim();
    if s.is_empty() || s.starts_with("https://") || s.starts_with("http://") {
        return Ok(());
    }
    Err(AppError {
        code: ErrorCode::InvalidQuery,
        message: "Bark 服务器的地址要以 http:// 或 https:// 开头".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn(session: &str, state: &str, requests: usize) -> ConnState {
        let mut c = ConnState::new("c1", None);
        c.session_id = Some(session.into());
        c.state = state.into();
        c.requests = vec![json!({}); requests];
        c
    }

    #[test]
    fn a_connected_sessions_status_is_ccrpcs_and_a_request_card_waits() {
        let live: BTreeMap<String, LiveStatus> = [
            ("a".to_string(), LiveStatus::Idle),
            ("b".to_string(), LiveStatus::Unknown),
        ]
        .into();
        let out = statuses(&live, &[conn("a", "busy", 0)]);
        assert_eq!(out["a"], Status::Busy);
        assert_eq!(out["b"], Status::Idle);
        assert_eq!(
            statuses(&live, &[conn("a", "busy", 1)])["a"],
            Status::Waiting
        );
        // A connection whose Session is not Live says nothing.
        assert!(!statuses(&live, &[conn("z", "waiting", 1)]).contains_key("z"));
        let mut ended = conn("a", "busy", 0);
        ended.session_exited = true;
        assert_eq!(statuses(&live, &[ended])["a"], Status::Idle);
    }

    #[test]
    fn a_viewers_last_stream_clears_what_it_looked_at() {
        let a = Arc::new(Attention::new(None));
        let phone = Viewer::Device("d1".into());
        let one = a.stream(phone.clone());
        let two = a.stream(phone.clone());
        a.set_viewing(phone.clone(), Some("s".into()));
        a.set_viewing(Viewer::Desktop, Some("t".into()));
        drop(one);
        assert_eq!(
            a.inner.lock().viewing.get(&phone).map(String::as_str),
            Some("s")
        );
        drop(two);
        let inner = a.inner.lock();
        assert!(!inner.viewing.contains_key(&phone));
        assert_eq!(inner.viewing[&Viewer::Desktop], "t");
    }

    #[test]
    fn a_bark_server_is_blank_or_http() {
        assert!(check_server("").is_ok() && check_server(" https://b.example").is_ok());
        assert!(check_server("b.example").is_err());
    }
}
