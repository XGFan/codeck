//! One connection's Connect state, folded from ccrpc's events: the part of the desktop app's
//! `src/features/connect/conn.ts` that every client of the Host shares. The events' own objects (requests, the
//! turn's end, progress, the waiting, tasks, the feedback draft) are kept as ccrpc wrote them.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnState {
    pub conn: String,
    pub holder_id: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub claude_version: Option<String>,
    pub ready: bool,
    /// `starting` / `idle` / `busy` / `waiting` / `exited`.
    pub state: String,
    pub waiting: Option<Value>,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub context_tokens: Option<u64>,
    /// What waits for an answer, as cards, in the order ccrpc reported them.
    pub requests: Vec<Value>,
    pub progress: Option<Value>,
    /// What waits in the TUI's own queue (ccrpc's queue items).
    pub queue: Vec<Value>,
    pub tasks: Vec<Value>,
    /// The last turn's end (`turn_end` as ccrpc wrote it).
    pub last_turn: Option<Value>,
    /// ccrpc's process ended (not the Session).
    pub proc_exit: Option<ProcExit>,
    pub session_exited: bool,
    /// Short notices, newest last, at most 5.
    pub notices: Vec<Notice>,
    pub feedback_draft: Option<Value>,
    /// The last `fork` event (`ccrpc new --fork` done, or why it failed), as ccrpc wrote it.
    pub fork: Option<Value>,
    /// A client of this Host disconnected it (its ccrpc's exit is not a failure).
    pub disconnected: bool,
}

/// The protocol version this Host understands.
const PROTOCOL: u64 = 4;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProcExit {
    pub code: Option<i32>,
    pub stderr: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: u64,
    pub text: String,
}

static NOTICE_SEQ: AtomicU64 = AtomicU64::new(0);

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// `null` / missing is none.
fn some(v: &Value) -> Option<Value> {
    (!v.is_null()).then(|| v.clone())
}

fn array(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

impl ConnState {
    pub fn new(conn: &str, holder_id: Option<String>) -> Self {
        ConnState {
            conn: conn.to_string(),
            holder_id,
            session_id: None,
            cwd: None,
            claude_version: None,
            ready: false,
            state: "starting".into(),
            waiting: None,
            mode: None,
            model: None,
            effort: None,
            context_tokens: None,
            requests: Vec::new(),
            progress: None,
            queue: Vec::new(),
            tasks: Vec::new(),
            last_turn: None,
            proc_exit: None,
            session_exited: false,
            notices: Vec::new(),
            feedback_draft: None,
            fork: None,
            disconnected: false,
        }
    }

    fn notice(&mut self, text: String) {
        if self.notices.len() >= 5 {
            self.notices.drain(..self.notices.len() - 4);
        }
        let id = NOTICE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        self.notices.push(Notice { id, text });
    }

    /// A request event: updates the request with the same `request_id` in place, else appends it.
    fn upsert_request(&mut self, r: Value) {
        match self
            .requests
            .iter_mut()
            .find(|x| x["request_id"] == r["request_id"])
        {
            Some(x) => *x = r,
            None => self.requests.push(r),
        }
    }

    /// Folds one ccrpc line (an event; responses and content change nothing). True when the state may have
    /// changed.
    pub fn apply(&mut self, m: &Value) -> bool {
        match m["type"].as_str().unwrap_or("") {
            "ready" => {
                self.ready = true;
                self.holder_id = text(&m["holder_id"]);
                self.session_id = text(&m["session_id"]);
                self.cwd = text(&m["cwd"]);
                self.claude_version = text(&m["claude_version"]);
                if let Some(s) = m["state"].as_str() {
                    self.state = s.to_string();
                }
                self.waiting = some(&m["waiting"]);
                self.mode = text(&m["mode"]);
                self.model = text(&m["model"]);
                self.effort = text(&m["effort"]);
                self.context_tokens = m["context_tokens"].as_u64();
                self.requests = array(&m["requests"]);
                self.queue = array(&m["queue"]);
                self.tasks = array(&m["tasks"]);
                self.feedback_draft = some(&m["feedback_draft"]);
                if m["protocol"].as_u64() != Some(PROTOCOL) {
                    let found = m["protocol"]
                        .as_u64()
                        .map_or_else(|| "未知".to_string(), |n| n.to_string());
                    self.notice(format!("ccrpc 版本不匹配：协议 {found}，需要 {PROTOCOL}"));
                }
            }
            "queue" => self.queue = array(&m["items"]),
            "tasks" => self.tasks = array(&m["tasks"]),
            "state" => {
                let state = m["state"].as_str().unwrap_or("").to_string();
                self.waiting = some(&m["waiting"]);
                if state != "busy" {
                    self.progress = None;
                }
                self.state = state;
            }
            "turn_end" => {
                self.progress = None;
                self.last_turn = Some(m.clone());
            }
            "permission_request" | "question" | "choice" | "rewind" | "dialog" | "unrecognized"
            | "btw" => self.upsert_request(m.clone()),
            "request_end" => self.requests.retain(|r| r["request_id"] != m["request_id"]),
            "session_info" => {
                self.mode = text(&m["mode"]);
                self.model = text(&m["model"]);
                self.effort = text(&m["effort"]);
                self.context_tokens = m["context_tokens"].as_u64();
            }
            "progress" => {
                if self.state != "busy" {
                    return false;
                }
                self.progress = Some(m.clone());
            }
            "feedback_draft" => self.feedback_draft = some(&m["draft"]),
            "follow_up_failed" => self.notice(format!(
                "排队的消息没有发出：{}",
                m["error"]["message"].as_str().unwrap_or("")
            )),
            "layout_learned" => self.notice(format!(
                "已学会新界面：{}",
                m["title"].as_str().unwrap_or("")
            )),
            "analysis_failed" => self.notice(format!(
                "界面识别失败：{}",
                m["error"].as_str().unwrap_or("")
            )),
            "session_changed" => {
                let session_id = text(&m["session_id"]);
                // `new`'s `ready` comes before claude has a Process File, without the cwd.
                let cwd = text(&m["cwd"]).or_else(|| self.cwd.take());
                // The first sessionId after `ready` (`new`): the same Session, nothing to reset.
                if m["previous"].is_null() {
                    self.session_id = session_id;
                    self.cwd = cwd;
                    return true;
                }
                let mut s = ConnState::new(&self.conn, self.holder_id.take());
                s.ready = true;
                s.session_id = session_id;
                s.cwd = cwd;
                s.claude_version = self.claude_version.take();
                s.state = std::mem::take(&mut self.state);
                s.waiting = self.waiting.take();
                s.mode = self.mode.take();
                s.model = self.model.take();
                s.effort = self.effort.take();
                s.disconnected = self.disconnected;
                *self = s;
            }
            "session_exit" => {
                self.session_exited = true;
                self.state = "exited".into();
                self.requests.clear();
                self.progress = None;
                self.feedback_draft = None;
            }
            "fork" => self.fork = Some(m.clone()),
            // Responses, and content events (block text, branch, workflow): not state.
            _ => return false,
        }
        true
    }

    /// ccrpc's process ended.
    pub fn proc_exit(&mut self, code: Option<i32>, stderr: String) {
        self.proc_exit = Some(ProcExit { code, stderr });
        self.requests.clear();
        self.progress = None;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fold(msgs: &[Value]) -> ConnState {
        fold_on(ConnState::new("c1", Some("h1".into())), msgs)
    }

    fn fold_on(mut s: ConnState, msgs: &[Value]) -> ConnState {
        for m in msgs {
            s.apply(m);
        }
        s
    }

    fn perm(id: &str, agent: Option<&str>) -> Value {
        json!({
            "type": "permission_request", "request_id": id, "tool_use_id": id, "tool_name": "Bash",
            "input": { "command": "ls" }, "agent_id": agent, "options": null, "layout_source": null,
            "from_code": false,
        })
    }

    fn end() -> Value {
        json!({
            "type": "turn_end", "prompt_id": "p", "trigger": "prompt", "last_text": "ok", "error": null,
            "aborted": false, "background_tasks": [], "duration_ms": 10,
            "usage": { "input_tokens": 1, "output_tokens": 1, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
            "api_cost": 0,
        })
    }

    fn ready() -> Value {
        json!({
            "type": "ready", "protocol": 4, "holder_id": "h1", "session_id": "old", "cwd": "/w",
            "claude_version": "2.1", "state": "idle", "waiting": null, "requests": [], "size": [80, 24],
            "mode": "manual", "model": "m", "context_tokens": 5, "queue": [], "tasks": [], "warnings": [],
        })
    }

    fn qi(text: &str) -> Value {
        json!({ "kind": "typed", "from": null, "text": text, "editable": true })
    }

    fn ids(s: &ConnState) -> Vec<&str> {
        s.requests
            .iter()
            .map(|r| r["request_id"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn keeps_the_feedback_draft_from_ready_or_its_event_until_gone_or_the_session_ended() {
        let draft = json!({ "title": "T", "tool_use_id": "toolu_1", "input": { "title": "T", "details": "- d" } });
        let mut r = ready();
        r["feedback_draft"] = draft.clone();
        let s = fold(&[r.clone()]);
        assert_eq!(s.feedback_draft, Some(draft));
        let gone = fold_on(
            s.clone(),
            &[json!({ "type": "feedback_draft", "draft": null })],
        );
        assert_eq!(gone.feedback_draft, None);
        let exited = fold_on(s, &[json!({ "type": "session_exit" })]);
        assert_eq!(exited.feedback_draft, None);
        assert_eq!(exited.state, "exited");
        assert!(exited.session_exited);
    }

    #[test]
    fn requests_live_until_request_end_whatever_the_state_does() {
        let waiting = json!({ "type": "state", "state": "waiting", "waiting": { "kind": "permission", "waiting_for": "permission prompt" } });
        let mut s = fold(&[waiting, perm("t1", None), perm("t2", Some("agent-1"))]);
        assert_eq!(s.state, "waiting");
        assert_eq!(s.waiting.as_ref().unwrap()["kind"], "permission");
        assert_eq!(ids(&s), ["t1", "t2"]);
        s.apply(&json!({ "type": "state", "state": "busy", "waiting": null }));
        assert_eq!(s.waiting, None);
        assert_eq!(ids(&s), ["t1", "t2"]);
        // Only `request_end` ends a request, and only its id.
        s.apply(&json!({ "type": "request_end", "request_id": "t2" }));
        assert_eq!(ids(&s), ["t1"]);
    }

    #[test]
    fn a_request_event_updates_the_request_with_its_id_in_place() {
        let mut again = perm("t1", None);
        again["options"] = json!([{ "label": "Yes", "meaning": "allow" }]);
        let s = fold(&[perm("t1", None), perm("t2", None), again]);
        assert_eq!(ids(&s), ["t1", "t2"]);
        assert_eq!(s.requests[0]["options"].as_array().unwrap().len(), 1);
        // Every kind of request is folded the same way, and none replaces another.
        let s = fold_on(
            s,
            &[
                json!({ "type": "dialog", "request_id": "dialog:1", "waiting_for": "x", "lines": ["a"] }),
                json!({ "type": "unrecognized", "request_id": "unrecognized:2", "context": "c", "lines": ["b"] }),
                json!({ "type": "choice", "request_id": "choice:3", "title": "t", "options": [] }),
                json!({ "type": "rewind", "request_id": "rewind:4", "uuid": "u", "text": "q", "code": null, "code_text": null, "warning": null, "options": [] }),
                json!({ "type": "btw", "request_id": "btw", "question": "q", "answering": true, "text": null }),
                json!({ "type": "dialog", "request_id": "dialog:1", "waiting_for": "x", "lines": ["b"] }),
            ],
        );
        assert_eq!(
            ids(&s),
            [
                "t1",
                "t2",
                "dialog:1",
                "unrecognized:2",
                "choice:3",
                "rewind:4",
                "btw"
            ]
        );
        assert_eq!(s.requests[2]["lines"], json!(["b"]));
    }

    #[test]
    fn ready_carries_the_requests_tasks_and_queue_and_a_wrong_protocol_is_a_notice() {
        let item = json!({ "kind": "typed", "from": null, "text": "a", "editable": true });
        let task = json!({ "id": "b1", "kind": "shell", "tool_use_id": null, "description": null, "command": "sleep 9", "status": "running", "idle": false });
        let mut r = ready();
        r["requests"] = json!([perm("t1", None)]);
        r["queue"] = json!([item]);
        r["tasks"] = json!([task]);
        let mut s = fold(&[r]);
        assert_eq!(ids(&s), ["t1"]);
        assert_eq!(s.queue, std::slice::from_ref(&item));
        assert_eq!(s.tasks, [task]);
        assert!(s.notices.is_empty());
        s.apply(&json!({ "type": "tasks", "tasks": [] }));
        assert!(s.tasks.is_empty());

        let mut old = ready();
        old["protocol"] = json!(3);
        let s = fold(&[old]);
        assert_eq!(s.notices.len(), 1);
        assert_eq!(s.notices[0].text, "ccrpc 版本不匹配：协议 3，需要 4");
    }

    #[test]
    fn keeps_the_last_turns_end_and_shows_progress_only_while_busy() {
        let progress = json!({ "type": "progress", "text": "Thinking…", "verb": "Thinking", "elapsed_s": 1, "tokens": null });
        let s = fold(std::slice::from_ref(&progress));
        assert_eq!(s.progress, None);
        let mut s = fold_on(
            s,
            &[
                json!({ "type": "state", "state": "busy" }),
                progress.clone(),
            ],
        );
        assert_eq!(s.progress, Some(progress));
        s.apply(&end());
        assert_eq!(s.progress, None);
        assert_eq!(s.last_turn, Some(end()));
    }

    #[test]
    fn starts_over_on_session_changed_keeping_what_describes_the_pane() {
        let mut s = fold(&[ready(), perm("t1", None), end()]);
        s.notice("n".into());
        s.apply(&json!({ "type": "session_changed", "session_id": "new", "previous": "old" }));
        assert_eq!(s.session_id.as_deref(), Some("new"));
        assert!(s.ready);
        assert_eq!(s.holder_id.as_deref(), Some("h1"));
        assert_eq!(s.cwd.as_deref(), Some("/w"));
        assert_eq!(s.mode.as_deref(), Some("manual"));
        assert_eq!(s.state, "idle");
        assert!(s.requests.is_empty());
        assert_eq!(s.last_turn, None);
        assert!(s.notices.is_empty());
        assert_eq!(s.context_tokens, None);

        // `new`: `ready` before the plugin checked in (and before the Process File); the first
        // sessionId only fills it in, with the cwd.
        let mut r = ready();
        r["session_id"] = Value::Null;
        r["cwd"] = Value::Null;
        let mut n = fold(&[r, perm("t1", None)]);
        n.apply(&json!({ "type": "session_changed", "session_id": "first", "previous": null, "cwd": "/n" }));
        assert_eq!(n.session_id.as_deref(), Some("first"));
        assert_eq!(n.cwd.as_deref(), Some("/n"));
        assert_eq!(ids(&n), ["t1"]);
    }

    #[test]
    fn keeps_the_fork_event_until_the_session_changes_and_the_disconnect_through_it() {
        let done = json!({ "type": "fork", "session_id": "f1", "origin": "s0", "text": "再试", "title": "T", "title_error": null });
        let mut s = fold(&[ready(), done.clone()]);
        assert_eq!(s.fork, Some(done));
        let failed =
            json!({ "type": "fork", "error": { "code": "exited", "message": "claude 退出了" } });
        s.apply(&failed);
        assert_eq!(s.fork, Some(failed));
        s.disconnected = true;
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["fork"]["error"]["code"], "exited");
        assert_eq!(v["disconnected"], true);
        s.apply(&json!({ "type": "session_changed", "session_id": "new", "previous": "old" }));
        assert_eq!(s.fork, None);
        assert!(s.disconnected);
        let v = serde_json::to_value(ConnState::new("c1", None)).unwrap();
        assert_eq!(
            (&v["fork"], &v["disconnected"]),
            (&Value::Null, &json!(false))
        );
    }

    #[test]
    fn ready_session_info_and_queue_fill_the_header() {
        let mut r = ready();
        r["queue"] = json!([qi("a")]);
        r["effort"] = json!("high");
        r["state"] = json!("waiting");
        r["waiting"] = json!({ "kind": "dialog", "waiting_for": "dialog open" });
        let mut s = fold(&[r]);
        assert_eq!(
            (s.ready, s.state.as_str(), s.claude_version.as_deref()),
            (true, "waiting", Some("2.1"))
        );
        assert_eq!(s.queue, [qi("a")]);
        assert_eq!(s.effort.as_deref(), Some("high"));
        assert_eq!(s.context_tokens, Some(5));
        s.apply(&json!({ "type": "queue", "items": [qi("a"), qi("b")] }));
        assert_eq!(s.queue, [qi("a"), qi("b")]);
        s.apply(&json!({ "type": "session_info", "mode": "plan", "model": "opus", "context_tokens": 9 }));
        assert_eq!(
            (s.mode.as_deref(), s.model.as_deref(), s.effort.as_deref()),
            (Some("plan"), Some("opus"), None)
        );
        assert_eq!(s.context_tokens, Some(9));
    }

    #[test]
    fn notices_keep_the_last_five() {
        let mut s = fold(&[]);
        for i in 0..7 {
            s.apply(&json!({ "type": "layout_learned", "layout": "l", "kind": "k", "title": format!("t{i}"), "path": "p" }));
        }
        s.apply(&json!({ "type": "follow_up_failed", "text": "x", "error": { "code": "c", "message": "忙" } }));
        s.apply(&json!({ "type": "analysis_failed", "error": "超时" }));
        let texts: Vec<_> = s.notices.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "已学会新界面：t4",
                "已学会新界面：t5",
                "已学会新界面：t6",
                "排队的消息没有发出：忙",
                "界面识别失败：超时"
            ]
        );
        assert!(s.notices.windows(2).all(|w| w[0].id < w[1].id));
    }

    #[test]
    fn serializes_camel_case_outside_and_raw_inside() {
        let mut s = fold(&[ready(), perm("t1", None)]);
        s.proc_exit(Some(1), "refused".into());
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["holderId"], "h1");
        assert_eq!(v["contextTokens"], 5);
        assert_eq!(v["procExit"], json!({ "code": 1, "stderr": "refused" }));
        assert_eq!(v["requests"], json!([]));
        assert_eq!(v["lastTurn"], Value::Null);
        assert_eq!(v["sessionExited"], false);
        let s = fold(&[perm("t1", None)]);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["requests"][0]["request_id"], "t1");
        assert_eq!(v["requests"][0]["tool_name"], "Bash");
    }
}
