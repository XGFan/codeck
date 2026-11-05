//! Turns: the plugin's hook events and the transcript's events opening and ending them, with
//! what each turn did (`turn_end`) and the markers that confirm commands.

use super::*;

/// What one turn did, reported with its `turn_end`.
#[derive(Default)]
pub(super) struct TurnStats {
    pub(super) started: Option<Instant>,
    pub(super) duration_ms: Option<u64>,
    pub(super) input: f64,
    pub(super) output: f64,
    pub(super) cache_read: f64,
    pub(super) cache_creation: f64,
    /// Estimated API cost; `None` once a message's model has no price.
    pub(super) cost: Option<f64>,
    pub(super) error: Option<Value>,
    pub(super) aborted: bool,
    /// Claude Code's id of the prompt (`None`: a turn already running when ccrpc connected).
    pub(super) prompt_id: Option<String>,
    /// `prompt`, or `task_notification` for a background task's result.
    pub(super) trigger: &'static str,
    /// The transcript wrote the turn's `turn_duration` marker.
    pub(super) marker_seen: bool,
    /// Opened for a steer expected to run after the previous Stop: dropped without `turn_end` if
    /// nothing comes of it.
    pub(super) provisional: bool,
}

impl TurnStats {
    pub(super) fn start(prompt_id: Option<String>, trigger: &'static str) -> TurnStats {
        TurnStats {
            started: Some(Instant::now()),
            cost: Some(0.0),
            prompt_id,
            trigger,
            ..Default::default()
        }
    }

    pub(super) fn to_json(&self) -> Value {
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

/// Transcript markers used to confirm commands, each with a monotonic sequence number (compare by
/// sequence, never by list length: the lists are bounded).
#[derive(Default)]
pub(super) struct Marks {
    pub(super) seq: u64,
    pub(super) prompts: VecDeque<(u64, String, String)>,
    pub(super) enqueued: VecDeque<(u64, String)>,
}

impl Marks {
    pub(super) fn prompt(&mut self, uuid: String, text: String) {
        self.seq += 1;
        self.prompts.push_back((self.seq, uuid, text));
        if self.prompts.len() > 64 {
            self.prompts.pop_front();
        }
    }

    pub(super) fn enqueue(&mut self, text: String) {
        self.seq += 1;
        self.enqueued.push_back((self.seq, text));
        if self.enqueued.len() > 64 {
            self.enqueued.pop_front();
        }
    }

    /// A slash command's record is rebuilt as `/name args`: spaces typed in it need not match.
    pub(super) fn prompt_after(&self, after: u64, text: &str) -> Option<&str> {
        let slash = text.trim_start().starts_with('/');
        self.prompts
            .iter()
            .rev()
            .find(|(s, _, t)| {
                *s > after
                    && (t.trim() == text.trim()
                        || (slash && recognize::collapse_ws(t) == recognize::collapse_ws(text)))
            })
            .map(|(_, u, _)| u.as_str())
    }

    /// A prompt sent with images: its recorded text may carry the `[Image #n]` placeholders.
    pub(super) fn prompt_with_images_after(&self, after: u64, text: &str) -> Option<&str> {
        self.prompts
            .iter()
            .rev()
            .find(|(s, _, t)| *s > after && t.trim().ends_with(text.trim()))
            .map(|(_, u, _)| u.as_str())
    }

    /// A prompt that went into Claude Code's queue: its text, or (sent with images) ending with it.
    pub(super) fn prompt_enqueued_after(&self, after: u64, text: &str, images: bool) -> bool {
        self.enqueued.iter().any(|(s, t)| {
            *s > after
                && if images {
                    t.trim().ends_with(text.trim())
                } else {
                    t.trim() == text.trim()
                }
        })
    }

    /// An enqueue recorded without its text: input submitted just as a turn began, or a message
    /// from another Claude Code session.
    pub(super) fn blank_enqueued_after(&self, after: u64) -> bool {
        self.enqueued
            .iter()
            .any(|(s, t)| *s > after && t.trim().is_empty())
    }
}

impl Engine {
    /// One observation step: process, Process File, transcript, state transitions, events.
    /// One transcript event: blocks out, turn bookkeeping.
    pub(super) fn on_tx(&mut self, ev: TxEvent) {
        match ev {
            TxEvent::Block(b) => {
                if b.agent_id.is_none() && b.kind == BlockKind::Text {
                    self.last_text = b.text.clone();
                }
                if b.agent_id.is_none()
                    && b.kind == BlockKind::User
                    && b.text
                        .as_deref()
                        .is_some_and(::transcript::assemble::prompt::is_interruption)
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
                    // Its Stop came: it ends here, before the blocks of a turn read in the same go.
                    if self.stop_pending.is_some() {
                        self.finish_stopped_turn();
                    }
                }
            }
            TxEvent::Setting { model, effort } => self.note_setting(model, effort),
            TxEvent::Usage {
                main,
                model,
                effort,
                usage,
                logged_cost,
            } => {
                if main {
                    self.note_main_usage(model.as_deref(), effort.as_deref(), &usage);
                }
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
            TxEvent::Queue(items) => self.set_queue(&items),
            TxEvent::Branch { at_uuid } => {
                self.emit(json!({"type": "branch", "at_uuid": at_uuid}));
            }
            TxEvent::Workflow(v) => self.emit(v),
        }
    }

    pub(super) fn on_hook(&mut self, m: HookMsg) {
        let i = &m.input;
        crate::clog!(
            "hook {} prompt={} tool={} tool_use={} agent={}",
            m.event,
            i["prompt_id"].as_str().unwrap_or("-"),
            i["tool_name"].as_str().unwrap_or("-"),
            i["tool_use_id"].as_str().unwrap_or("-"),
            i["agent_id"].as_str().unwrap_or("-")
        );
        if let Some(mode) = i["permission_mode"].as_str()
            && i["agent_id"].is_null()
        {
            self.hook_mode = Some(mode.to_string());
        }
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
                let text =
                    ::transcript::assemble::prompt::unwrap_pasted(&s("prompt").unwrap_or_default());
                self.hook_seq += 1;
                self.hook_prompts
                    .push_back((self.hook_seq, pid.clone(), text.clone()));
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
                let trigger = if notice {
                    "task_notification"
                } else {
                    "prompt"
                };
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
                // Another turn's Stop (a steer's turn) while this one still waits for the transcript:
                // claude fires both before writing either turn, so this one ends at its marker (or
                // after STOP_CATCH_UP) with all its blocks, and the other Stop is handled then.
                if self.stop_pending.is_some() && pid.is_some() && self.turn.prompt_id != pid {
                    // A third one: the oldest waits no longer.
                    if let Some(prev) = self.deferred_stop.take() {
                        self.end_stopped_turn();
                        self.on_hook(prev);
                    }
                    crate::clog!("defer Stop prompt={}", pid.as_deref().unwrap_or("-"));
                    self.deferred_stop = Some(m);
                    return;
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
                    self.turn.error =
                        Some(json!({"kind": s("error"), "message": s("last_assistant_message")}));
                } else if let Some(t) = s("last_assistant_message") {
                    self.last_text = Some(t);
                }
                let bg = i.get("background_tasks").cloned().unwrap_or(json!([]));
                self.note_running_tasks(&bg);
                let last = if m.event == "Stop" {
                    self.last_text.clone()
                } else {
                    None
                };
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
                    if self
                        .hook_tasks
                        .iter()
                        .any(|t| t.id == id && t.status == "running")
                    {
                        self.set_task_status(&id, "completed");
                    }
                }
            }
            _ => {}
        }
    }

    /// `turn_end` of the turn whose Stop came, if any; then the next turn's Stop if it was deferred.
    pub(super) fn finish_stopped_turn(&mut self) {
        self.end_stopped_turn();
        if let Some(m) = self.deferred_stop.take() {
            self.on_hook(m);
        }
    }

    pub(super) fn end_stopped_turn(&mut self) {
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

    pub(super) fn emit_turn_end(&mut self, last_text: Option<String>, background_tasks: Value) {
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

    pub(super) fn note_main_usage(
        &mut self,
        model: Option<&str>,
        effort: Option<&str>,
        u: &::transcript::pricing::Usage,
    ) {
        if let Some(m) = model.filter(|m| !m.is_empty() && *m != "<synthetic>") {
            self.model = Some(m.to_string());
        }
        // Older versions record none: keep what the screen said.
        if let Some(e) = effort.filter(|e| !e.is_empty()) {
            self.effort = Some(e.to_string());
        }
        let ctx = u.input + u.cache_read + u.cache_write_5m + u.cache_write_1h;
        if ctx > 0.0 {
            self.context_tokens = Some(ctx as u64);
        }
    }

    /// `/model` or `/effort` set something (here or in the TUI). A new model may not have the
    /// old effort level, or any: the screen's mark says again.
    pub(super) fn note_setting(&mut self, model: Option<String>, effort: Option<String>) {
        if let Some(m) = model {
            if self.model.as_deref() != Some(m.as_str()) {
                self.effort = None;
            }
            self.model = Some(m);
        }
        if let Some(e) = effort {
            self.effort = Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slash_command_confirms_whatever_spaces_were_typed() {
        let mut m = Marks::default();
        m.prompt("u1".into(), "/release v1".into());
        assert_eq!(m.prompt_after(0, "/release  v1 "), Some("u1"));
        assert_eq!(m.prompt_after(1, "/release v1"), None);
        m.prompt("u2".into(), "hello world".into());
        assert_eq!(m.prompt_after(0, "hello  world"), None);
    }

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
        assert!(m.prompt_enqueued_after(b2, "steer", false));
        assert!(!m.prompt_enqueued_after(b2, "s5", false));
        // Typed just as a turn began, the enqueue is recorded without its text.
        let b3 = m.seq;
        assert!(!m.blank_enqueued_after(b3));
        m.enqueue(String::new());
        assert!(m.blank_enqueued_after(b3));
        assert!(!m.prompt_enqueued_after(b3, "late", false));
        // With images, the queued text carries the `[Image #n]` placeholders first.
        m.enqueue("[Image #1] look".into());
        assert!(m.prompt_enqueued_after(b3, "look", true));
        assert!(!m.prompt_enqueued_after(b3, "look", false));
    }
}
