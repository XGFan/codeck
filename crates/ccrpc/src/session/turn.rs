//! Turns: the plugin's hook events and the transcript's events opening and ending them, with
//! what each turn did (`turn_end`) ([`TurnTracker`], no IO), and the markers that confirm
//! commands.

use super::*;

/// The input box shows `prompt`: whole, or (a long one) at least its first 20 characters;
/// whitespace aside (the box wraps).
fn shows_prompt(shown: &str, prompt: &str) -> bool {
    let squeeze = |t: &str| t.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let (shown, whole) = (squeeze(shown), squeeze(prompt));
    !shown.is_empty()
        && (shown == whole || (shown.chars().count() >= 20 && whole.starts_with(&shown)))
}

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
    /// The prompt's text, from UserPromptSubmit.
    pub(super) prompt_text: Option<String>,
    /// `prompt`, or `task_notification` for a background task's result.
    pub(super) trigger: &'static str,
    /// The transcript wrote the turn's `turn_duration` marker.
    pub(super) marker_seen: bool,
    /// Opened for a steer expected to run after the previous Stop: dropped without `turn_end` if
    /// nothing comes of it.
    pub(super) provisional: bool,
    /// Goes on in a background claude (the conversation moved there): ended by its next Stop,
    /// whatever the prompt id.
    pub(super) moved: bool,
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
    /// Sequence, uuid, text and image count of each user prompt.
    pub(super) prompts: VecDeque<(u64, String, String, usize)>,
    pub(super) enqueued: VecDeque<(u64, String)>,
}

impl Marks {
    pub(super) fn prompt(&mut self, uuid: String, text: String, images: usize) {
        self.seq += 1;
        self.prompts.push_back((self.seq, uuid, text, images));
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

    /// A slash or `!` command's record is rebuilt as `/name args` / `! cmd`: spaces typed in it
    /// need not match, nor a plugin's name the command was typed without (see [`same_command`]).
    pub(super) fn prompt_after(&self, after: u64, text: &str) -> Option<&str> {
        let slash = text.trim_start().starts_with(['/', '!']);
        self.prompts
            .iter()
            .rev()
            .find(|(s, _, t, _)| {
                *s > after && (t.trim() == text.trim() || (slash && same_command(t, text)))
            })
            .map(|(_, u, _, _)| u.as_str())
    }

    /// A prompt sent with images: its recorded text may carry the `[Image #n]` placeholders.
    pub(super) fn prompt_with_images_after(&self, after: u64, text: &str) -> Option<&str> {
        self.prompts
            .iter()
            .rev()
            .find(|(s, _, t, _)| *s > after && t.trim().ends_with(text.trim()))
            .map(|(_, u, _, _)| u.as_str())
    }

    /// A prompt sent as `parts` with images: it carries that many images, its text has an
    /// `[Image #n]` where each went (see [`parts_match`]).
    pub(super) fn parts_prompt_after(&self, after: u64, text: &str, images: usize) -> Option<&str> {
        self.prompts
            .iter()
            .rev()
            .find(|(s, _, t, n)| *s > after && *n == images && parts_match(t, text, Some(images)))
            .map(|(_, u, _, _)| u.as_str())
    }

    /// A prompt sent as `parts` with images that went into Claude Code's queue: the queued text
    /// shows each image as its `[Image #n]`.
    pub(super) fn parts_enqueued_after(&self, after: u64, text: &str, images: usize) -> bool {
        self.enqueued
            .iter()
            .any(|(s, t)| *s > after && parts_match(t, text, Some(images)))
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

/// What a `prompt` or `steer` typed, to find it in the transcript and the input box.
pub(super) struct Typed {
    /// The text without control characters: `text`, or the parts' text joined.
    pub(super) text: String,
    pub(super) images: usize,
    /// Sent as `parts` with images: the record has them among the text.
    pub(super) parts: bool,
    /// The start of its first text (or an image tag): still in the input box after Enter, it
    /// was not submitted.
    pub(super) needle: String,
}

impl Typed {
    /// `text` after `images` images.
    pub(super) fn plain(text: String, images: usize) -> Typed {
        let needle = Self::start(&text);
        Typed {
            text,
            images,
            parts: false,
            needle,
        }
    }

    /// Checked `parts` with at least one image.
    pub(super) fn parts(parts: &[rpc::Part]) -> Typed {
        let input = rpc::Input::Parts(parts.to_vec());
        let first = parts.iter().find_map(|p| match p {
            rpc::Part::Text(t) if !t.trim().is_empty() => Some(t.as_str()),
            _ => None,
        });
        Typed {
            text: input.typed_text(),
            images: parts
                .iter()
                .filter(|p| matches!(p, rpc::Part::Image(_)))
                .count(),
            parts: true,
            needle: first.map_or_else(|| "[Image #".to_string(), Self::start),
        }
    }

    fn start(text: &str) -> String {
        text.trim()
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(20)
            .collect()
    }

    /// The prompt the plugin's UserPromptSubmit reported is this one.
    pub(super) fn hook_matches(&self, reported: &str) -> bool {
        if self.parts {
            parts_match(reported, &self.text, None)
        } else {
            reported.trim().ends_with(self.text.trim())
        }
    }
}

impl Marks {
    /// The user prompt `typed` sent, recorded after `after`.
    pub(super) fn typed_after(&self, after: u64, typed: &Typed) -> Option<&str> {
        if typed.parts {
            self.parts_prompt_after(after, &typed.text, typed.images)
        } else if typed.images > 0 {
            self.prompt_with_images_after(after, &typed.text)
        } else {
            self.prompt_after(after, &typed.text)
        }
    }

    /// `typed` went into Claude Code's queue after `after`.
    pub(super) fn typed_enqueued_after(&self, after: u64, typed: &Typed) -> bool {
        if typed.parts {
            self.parts_enqueued_after(after, &typed.text, typed.images)
        } else {
            self.prompt_enqueued_after(after, &typed.text, typed.images > 0)
        }
    }
}

/// A command's record (`recorded`, as `/name args` or `! cmd`) and what was typed: the same once
/// whitespace runs are folded, or a plugin's command typed by its bare name, which Claude Code
/// records with the plugin's (`/autopilot x` as `/oh-my-claudecode:autopilot x`).
fn same_command(recorded: &str, typed: &str) -> bool {
    let (r, t) = (
        recognize::collapse_ws(recorded),
        recognize::collapse_ws(typed),
    );
    if r == t {
        return true;
    }
    let (Some(r), Some(t)) = (r.strip_prefix('/'), t.strip_prefix('/')) else {
        return false;
    };
    let (r_name, r_args) = r.split_once(' ').unwrap_or((r, ""));
    let (t_name, t_args) = t.split_once(' ').unwrap_or((t, ""));
    r_args == t_args
        && !t_name.contains(':')
        && r_name
            .rsplit_once(':')
            .is_some_and(|(plugin, name)| !plugin.is_empty() && name == t_name)
}

/// Text recorded for a prompt sent as `parts` (`typed`: the parts' text joined): the same once its
/// `[Image #n]` tags and all whitespace are left out (Claude Code numbers the tags itself and may
/// set long pastes apart with blank lines). With `images`, the record has that many tags more
/// than the typed text.
pub(super) fn parts_match(recorded: &str, typed: &str, images: Option<usize>) -> bool {
    let (r, rn) = recognize::without_image_tags(recorded);
    let (t, tn) = recognize::without_image_tags(typed);
    images.is_none_or(|n| rn == tn + n) && recognize::same_text(&r, &t)
}

/// What turn bookkeeping asks of the engine, in order.
#[derive(Debug, PartialEq)]
pub(super) enum TurnOut {
    /// A turn opened: idle time before now is not its.
    Opened,
    /// A Stop was taken as the running turn's: the background tasks it lists.
    Stopped(Value),
    /// A `turn_end` event to emit, with the turn's prompt (as the plugin reported it).
    End(Value, Option<String>),
}

/// How soon a `!` command ccrpc ran shows in the transcript (it is written as the command starts).
const BANG_RECORD: Duration = Duration::from_secs(10);

/// A Stop or StopFailure from the plugin, as kept while it waits for its turn.
struct Stop {
    prompt_id: Option<String>,
    failure: bool,
    error: Option<String>,
    last_message: Option<String>,
    background_tasks: Value,
}

/// Turns without IO: fed the plugin's hook events and the transcript's events (with the time they
/// are handled), it yields what the engine does next ([`TurnOut`]). A turn opens on
/// UserPromptSubmit and ends on its Stop / StopFailure once the transcript caught up, or as
/// aborted when the Session went idle without one.
pub(super) struct TurnTracker {
    /// A turn is running (opened by UserPromptSubmit, or running at connect) and has not ended.
    active: bool,
    stats: TurnStats,
    /// The main line's last text (a Stop's `last_assistant_message` takes over).
    last_text: Option<String>,
    /// Input queued during the turn (steers) not yet delivered within it: delivered after its
    /// Stop as a turn of its own, which has no UserPromptSubmit.
    pending_steers: Vec<String>,
    /// The turn's Stop came: `(when, last text, background tasks)`; `turn_end` once the transcript
    /// caught up.
    stop_pending: Option<(Instant, Option<String>, Value)>,
    /// The next turn's Stop, come while this one's still waits for the transcript (claude fires
    /// both Stops before writing either turn): handled once this turn has ended.
    deferred_stop: Option<Stop>,
    /// When ccrpc ran a `!` command (its turn already open): its transcript record, coming within
    /// [`BANG_RECORD`], opens nothing.
    bang_sent: Option<Instant>,
    prices: ::transcript::pricing::PriceTable,
}

impl TurnTracker {
    pub(super) fn new() -> TurnTracker {
        TurnTracker {
            active: false,
            stats: TurnStats::default(),
            last_text: None,
            pending_steers: Vec::new(),
            stop_pending: None,
            deferred_stop: None,
            bang_sent: None,
            prices: ::transcript::pricing::PriceTable::embedded(),
        }
    }

    /// A turn is running (opened by UserPromptSubmit, or running at connect) and has not ended.
    pub(super) fn active(&self) -> bool {
        self.active
    }

    /// A turn already running at connect: its Stop (whatever its prompt id) ends it.
    pub(super) fn adopt_running(&mut self) {
        self.active = true;
        self.stats = TurnStats::start(None, "prompt");
    }

    /// The conversation moved to a background claude (agents page): a turn running then goes on
    /// there, with no UserPromptSubmit and under that claude's prompt id, so its Stop (whatever its
    /// prompt id) ends it; idle there without one, it ends as aborted.
    pub(super) fn moved(&mut self) {
        if self.active && self.stop_pending.is_none() {
            self.stats.moved = true;
        }
    }

    /// No turn running at connect.
    pub(super) fn idle_at_connect(&mut self) {
        self.active = false;
    }

    /// The main line's text read as history (at connect, on switching Session).
    pub(super) fn note_history_text(&mut self, text: Option<String>) {
        self.last_text = text;
    }

    /// The plugin's UserPromptSubmit, Stop, StopFailure and PreToolUse (other events: nothing).
    /// `busy`: the Session is working (busy or waiting) as the hook arrives.
    pub(super) fn on_hook(&mut self, m: &HookMsg, now: Instant, busy: bool) -> Vec<TurnOut> {
        let i = &m.input;
        let s = |k: &str| i[k].as_str().map(str::to_string);
        match m.event.as_str() {
            "UserPromptSubmit" => {
                let text =
                    ::transcript::assemble::prompt::unwrap_pasted(&s("prompt").unwrap_or_default());
                self.prompt_submitted(s("prompt_id").unwrap_or_default(), text, now, busy)
            }
            "Stop" | "StopFailure" => self.stop(
                Stop {
                    prompt_id: s("prompt_id"),
                    failure: m.event == "StopFailure",
                    error: s("error"),
                    last_message: s("last_assistant_message"),
                    // Null when missing: does not end the tasks known only from Stop.
                    background_tasks: i.get("background_tasks").cloned().unwrap_or(Value::Null),
                },
                now,
            ),
            "PreToolUse" => {
                // A turn opened without UserPromptSubmit (a steer delivered after Stop) learns
                // its prompt id here.
                if self.active && self.stats.prompt_id.is_none() && i["agent_id"].is_null() {
                    self.stats.prompt_id = s("prompt_id");
                    self.stats.provisional = false;
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn prompt_submitted(
        &mut self,
        pid: String,
        text: String,
        now: Instant,
        busy: bool,
    ) -> Vec<TurnOut> {
        let notice = text.trim_start().starts_with("<task-notification>");
        let mut out = self.finish_stopped(now);
        // A steer's own turn has no UserPromptSubmit: one now is a new prompt.
        if self.active && self.stats.provisional {
            self.active = false;
            self.stats = TurnStats::default();
        }
        if self.active {
            match &self.stats.prompt_id {
                // Input while busy is delivered within the same turn (steer), or after its Stop
                // as a turn of its own.
                // Moved to a background claude: its input while it works on the turn carries its
                // own prompt id. Idle there, a prompt is a new one.
                Some(p) if *p == pid || (self.stats.moved && busy) => {
                    if !notice {
                        self.pending_steers.push(text);
                    }
                    return out;
                }
                // A turn running at connect: a prompt during it is a steer of it.
                None => {
                    self.stats.prompt_id = Some(pid);
                    if !notice {
                        self.pending_steers.push(text);
                    }
                    return out;
                }
                // The previous turn never got its Stop. What the input box shows now is not
                // what that turn left there (the new prompt may be the same words sent again).
                Some(_) => {
                    self.stats.aborted = true;
                    self.stats.prompt_text = None;
                    out.push(self.end(None, Value::Null));
                }
            }
        }
        let trigger = if notice {
            "task_notification"
        } else {
            "prompt"
        };
        self.pending_steers.clear();
        self.stats = TurnStats::start(Some(pid), trigger);
        self.stats.prompt_text = (!notice).then_some(text);
        self.active = true;
        self.last_text = None;
        out.push(TurnOut::Opened);
        out
    }

    fn stop(&mut self, stop: Stop, now: Instant) -> Vec<TurnOut> {
        let mut out = Vec::new();
        let pid = stop.prompt_id.clone();
        // Another turn's Stop (a steer's turn) while this one still waits for the transcript:
        // claude fires both before writing either turn, so this one ends at its marker (or after
        // STOP_CATCH_UP) with all its blocks, and the other Stop is handled then.
        if self.stop_pending.is_some() && pid.is_some() && self.stats.prompt_id != pid {
            // A third one: the oldest waits no longer.
            if let Some(prev) = self.deferred_stop.take() {
                out.extend(self.end_stopped());
                out.extend(self.stop(prev, now));
            }
            crate::clog!("defer Stop prompt={}", pid.as_deref().unwrap_or("-"));
            self.deferred_stop = Some(stop);
            return out;
        }
        let ours = self.active
            && self.stop_pending.is_none()
            && (self.stats.prompt_id.is_none() || self.stats.prompt_id == pid || self.stats.moved);
        // A turn ccrpc did not see start.
        if !ours {
            return out;
        }
        // A turn opened without its prompt id (running at connect, a steer delivered after Stop)
        // takes the Stop's.
        if self.stats.prompt_id.is_none() {
            self.stats.prompt_id = pid;
        }
        if stop.failure {
            self.stats.error = Some(json!({"kind": stop.error, "message": stop.last_message}));
        } else if let Some(t) = stop.last_message {
            self.last_text = Some(t);
        }
        out.push(TurnOut::Stopped(stop.background_tasks.clone()));
        let last = if stop.failure {
            None
        } else {
            self.last_text.clone()
        };
        self.stop_pending = Some((now, last, stop.background_tasks));
        out
    }

    /// One transcript event: what the turn did, and the marker that it is all written.
    pub(super) fn on_tx(&mut self, ev: &TxEvent, now: Instant) -> Vec<TurnOut> {
        match ev {
            TxEvent::Block(b) if b.agent_id.is_none() => {
                if b.kind == BlockKind::Text {
                    self.last_text = b.text.clone();
                }
                if b.kind == BlockKind::User {
                    if b.text
                        .as_deref()
                        .is_some_and(::transcript::assemble::prompt::is_interruption)
                    {
                        self.stats.aborted = true;
                    }
                    // A steer delivered within the turn.
                    if b.queued {
                        let t = b.text.as_deref().unwrap_or_default().trim();
                        self.pending_steers.retain(|s| s.trim() != t);
                    }
                }
            }
            // Turns are delimited by the plugin's UserPromptSubmit / Stop; the marker says the
            // transcript has the turn's last message.
            TxEvent::TurnEnd { duration_ms } => {
                // The first marker is the turn's (a steer's own turn may follow in the same read).
                if self.active && !self.stats.marker_seen {
                    self.stats.marker_seen = true;
                    if duration_ms.is_some() {
                        self.stats.duration_ms = *duration_ms;
                    }
                    // Its Stop came: it ends here, before the blocks of a turn read in the same go.
                    if self.stop_pending.is_some() {
                        return self.finish_stopped(now);
                    }
                }
            }
            TxEvent::Usage {
                model,
                usage,
                logged_cost,
                ..
            } if self.active => {
                let t = &mut self.stats;
                t.input += usage.input;
                t.output += usage.output;
                t.cache_read += usage.cache_read;
                t.cache_creation += usage.cache_write_5m + usage.cache_write_1h;
                let c = self.prices.cost(model.as_deref(), usage, *logged_cost);
                t.cost = t.cost.zip(c).map(|(a, b)| a + b);
            }
            // A `!` command has no UserPromptSubmit, yet runs as a turn (its output goes to the
            // model) ended by a Stop: one typed in the TUI opens here (its Stop may have come
            // first: then dropped without `turn_end`); ccrpc's own opened before Enter.
            TxEvent::UserPrompt { text, .. } if text.starts_with("! ") => {
                let ours = self
                    .bang_sent
                    .take()
                    .is_some_and(|t| now.duration_since(t) < BANG_RECORD);
                if ours || self.active {
                    return Vec::new();
                }
                self.open_provisional();
                return vec![TurnOut::Opened];
            }
            TxEvent::ApiError { kind, message } => {
                if self.active && self.stats.error.is_none() {
                    self.stats.error = Some(json!({"kind": kind, "message": message}));
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// The time-driven ends: a stopped turn once the transcript caught up (or after
    /// [`STOP_CATCH_UP`]); a turn the Session has been idle in without a Stop for [`ABORT_GRACE`]
    /// (`idle_since`: when it went idle, `None` while not idle).
    pub(super) fn poll(&mut self, now: Instant, mut idle_since: Option<Instant>) -> Vec<TurnOut> {
        let mut out = Vec::new();
        if let Some((at, _, _)) = &self.stop_pending
            && (self.stats.marker_seen || now.duration_since(*at) >= STOP_CATCH_UP)
        {
            out = self.finish_stopped(now);
            // A turn opened just now counts its idle afresh.
            if out.contains(&TurnOut::Opened) && idle_since.is_some() {
                idle_since = Some(now);
            }
        }
        // Stop comes before idle: a turn that went idle without one was interrupted.
        if self.active
            && self.stop_pending.is_none()
            && idle_since.is_some_and(|t| {
                let grace = if self.stats.moved {
                    MOVED_GRACE
                } else {
                    ABORT_GRACE
                };
                now.duration_since(t) >= grace
            })
        {
            if self.stats.provisional {
                // The steer had been delivered within the previous turn after all.
                self.active = false;
                self.stats = TurnStats::default();
            } else {
                self.stats.aborted = true;
                out.push(self.end(None, Value::Null));
            }
        }
        out
    }

    /// `send_now` (`ctrl+enter`): the running turn ends here as aborted (an interrupted turn gets
    /// no Stop); Claude Code's queue runs as a turn of its own with no UserPromptSubmit, ended by
    /// its Stop (as a steer delivered after Stop).
    pub(super) fn send_now(&mut self) -> Vec<TurnOut> {
        let mut out = Vec::new();
        if self.active {
            self.stats.aborted = true;
            out.push(self.end(None, Value::Null));
        }
        self.pending_steers.clear();
        self.open_provisional();
        out
    }

    /// `turn_end` of the turn whose Stop came, if any; then the next turn's Stop if it was deferred.
    fn finish_stopped(&mut self, now: Instant) -> Vec<TurnOut> {
        let mut out = self.end_stopped();
        if let Some(stop) = self.deferred_stop.take() {
            out.extend(self.stop(stop, now));
        }
        out
    }

    fn end_stopped(&mut self) -> Vec<TurnOut> {
        let Some((_, last, bg)) = self.stop_pending.take() else {
            return Vec::new();
        };
        let mut out = vec![self.end(last, bg)];
        // Input queued in it but not delivered within it runs next, with no UserPromptSubmit:
        // open that turn now (its Stop ends it).
        if !self.pending_steers.is_empty() {
            self.pending_steers.clear();
            self.open_provisional();
            out.push(TurnOut::Opened);
        }
        out
    }

    /// ccrpc is about to run a `!` command (no UserPromptSubmit comes): its turn opens before
    /// Enter, as its Stop may come before the transcript shows the command.
    pub(super) fn bang_sent(&mut self) -> Vec<TurnOut> {
        self.bang_sent = Some(Instant::now());
        if self.active {
            return Vec::new();
        }
        self.open_provisional();
        vec![TurnOut::Opened]
    }

    /// A turn expected to run with no UserPromptSubmit: dropped without `turn_end` if nothing
    /// comes of it.
    fn open_provisional(&mut self) {
        self.stats = TurnStats::start(None, "prompt");
        self.stats.provisional = true;
        self.active = true;
        self.last_text = None;
    }

    fn end(&mut self, last_text: Option<String>, background_tasks: Value) -> TurnOut {
        self.active = false;
        let mut v = self.stats.to_json();
        v["type"] = json!("turn_end");
        v["last_text"] = json!(last_text);
        v["background_tasks"] = match background_tasks {
            Value::Array(a) => Value::Array(a),
            _ => json!([]),
        };
        let prompt = self.stats.prompt_text.take();
        self.stats = TurnStats::default();
        TurnOut::End(v, prompt)
    }
}

impl Engine {
    /// One transcript event: turn bookkeeping, then blocks and the rest out.
    pub(super) fn on_tx(&mut self, ev: TxEvent) {
        let out = self.turns.on_tx(&ev, Instant::now());
        self.apply_turns(out);
        match ev {
            TxEvent::Block(b) => {
                let mut v = serde_json::to_value(&b).unwrap_or(Value::Null);
                if let Value::Object(m) = &mut v {
                    m.insert("type".into(), json!("block"));
                }
                self.emit(v);
            }
            TxEvent::TurnEnd { .. } | TxEvent::ApiError { .. } => {}
            TxEvent::Setting { model, effort } => self.note_setting(model, effort),
            TxEvent::Usage {
                main,
                model,
                effort,
                usage,
                ..
            } => {
                if main {
                    self.note_main_usage(model.as_deref(), effort.as_deref(), &usage);
                }
            }
            TxEvent::UserPrompt { uuid, text, images } => {
                // A person's prompt after a rewind (a resend of the same one forks no Branch):
                // the transcript's conversation is the TUI's again (a `!` command too). Slash commands and task
                // notifications are not.
                if !text.starts_with('/') && transcript::task_notification(&text).is_none() {
                    self.rewound = None;
                }
                self.marks.prompt(uuid, text, images)
            }
            TxEvent::Enqueued { content } => self.marks.enqueue(content),
            TxEvent::Queue(items) => self.set_queue(&items),
            TxEvent::Branch { at_uuid } => {
                self.rewound = None;
                self.emit(json!({"type": "branch", "at_uuid": at_uuid}));
            }
            TxEvent::Workflow(v) => self.emit(v),
        }
    }

    /// Carries out what turn bookkeeping yielded, in order.
    pub(super) fn apply_turns(&mut self, out: Vec<TurnOut>) {
        for o in out {
            match o {
                // Idle until now is not this turn's: count afresh.
                TurnOut::Opened => {
                    if self.idle_at.is_some() {
                        self.idle_at = Some(Instant::now());
                    }
                }
                TurnOut::Stopped(bg) => self.note_running_tasks(&bg),
                TurnOut::End(mut v, prompt) => {
                    v["restored_text"] = json!(self.restored(&v, prompt));
                    self.emit(v)
                }
            }
        }
    }

    /// An aborted turn's prompt when Claude Code put it back in its input box (Esc before any
    /// reply takes it out of the conversation): the input box shows it, whole or (a long one) at
    /// least its first 20 characters.
    fn restored(&self, v: &Value, prompt: Option<String>) -> Option<String> {
        let prompt = prompt.filter(|_| v["aborted"] == true)?;
        let shown = self.screen().ok().and_then(|s| recognize::input_text(&s))?;
        shows_prompt(&shown, &prompt).then_some(prompt)
    }

    pub(super) fn on_hook(&mut self, m: HookMsg) {
        let busy = matches!(self.state, State::Busy | State::Waiting { .. });
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
                        self.switch_to(sid, &path, history, HashSet::new());
                    }
                }
            }
            "UserPromptSubmit" => {
                let pid = s("prompt_id").unwrap_or_default();
                let text =
                    ::transcript::assemble::prompt::unwrap_pasted(&s("prompt").unwrap_or_default());
                if let Some((id, status)) = transcript::task_notification(&text) {
                    self.set_task_status(&id, &status);
                }
                self.hook_seq += 1;
                self.hook_prompts.push_back((self.hook_seq, pid, text));
                if self.hook_prompts.len() > 64 {
                    self.hook_prompts.pop_front();
                }
                let out = self.turns.on_hook(&m, Instant::now(), busy);
                self.apply_turns(out);
            }
            "Stop" | "StopFailure" => {
                let out = self.turns.on_hook(&m, Instant::now(), busy);
                self.apply_turns(out);
            }
            "PreToolUse" => {
                let out = self.turns.on_hook(&m, Instant::now(), busy);
                self.apply_turns(out);
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
    use crate::transcript::Block;

    fn hook(t: &mut TurnTracker, now: Instant, event: &str, input: Value) -> Vec<TurnOut> {
        let m = HookMsg {
            event: event.into(),
            input,
            reply: None,
        };
        t.on_hook(&m, now, true)
    }

    fn submit(t: &mut TurnTracker, now: Instant, pid: &str, text: &str) -> Vec<TurnOut> {
        hook(
            t,
            now,
            "UserPromptSubmit",
            json!({"prompt_id": pid, "prompt": text}),
        )
    }

    fn stop(t: &mut TurnTracker, now: Instant, pid: &str, last: &str) -> Vec<TurnOut> {
        hook(
            t,
            now,
            "Stop",
            json!({"prompt_id": pid, "last_assistant_message": last, "background_tasks": []}),
        )
    }

    fn marker(t: &mut TurnTracker, now: Instant, ms: u64) -> Vec<TurnOut> {
        t.on_tx(
            &TxEvent::TurnEnd {
                duration_ms: Some(ms),
            },
            now,
        )
    }

    fn block(kind: BlockKind, text: &str, queued: bool) -> TxEvent {
        TxEvent::Block(Box::new(Block {
            kind,
            uuid: "u".into(),
            index: 0,
            text: Some(text.into()),
            id: None,
            name: None,
            input: None,
            tool_use_id: None,
            content: None,
            is_error: None,
            persisted: None,
            queued,
            images: Vec::new(),
            agent_id: None,
            parent_tool_use_id: None,
            run_id: None,
        }))
    }

    /// The `turn_end` events among `out`.
    fn ends(out: &[TurnOut]) -> Vec<&Value> {
        out.iter()
            .filter_map(|o| match o {
                TurnOut::End(v, _) => Some(v),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_turn_ends_at_its_marker_after_stop_with_what_it_did() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        assert_eq!(submit(&mut t, now, "p1", "hi"), vec![TurnOut::Opened]);
        let usage = ::transcript::pricing::Usage {
            input: 10.0,
            output: 3.0,
            ..Default::default()
        };
        let u = TxEvent::Usage {
            main: true,
            model: None,
            effort: None,
            usage,
            logged_cost: Some(0.5),
        };
        t.on_tx(&u, now);
        t.on_tx(&u, now);
        t.on_tx(&block(BlockKind::Text, "hello", false), now);
        let out = hook(
            &mut t,
            now,
            "Stop",
            json!({"prompt_id": "p1", "last_assistant_message": "hello there",
                "background_tasks": [{"id": "b1", "type": "shell"}]}),
        );
        assert_eq!(
            out,
            vec![TurnOut::Stopped(json!([{"id": "b1", "type": "shell"}]))]
        );
        // Not before the transcript has the turn (or STOP_CATCH_UP passed).
        assert!(t.poll(now + Duration::from_millis(100), None).is_empty());
        let out = marker(&mut t, now, 1234);
        let e = ends(&out);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0]["prompt_id"], "p1");
        assert_eq!(e[0]["trigger"], "prompt");
        assert_eq!(e[0]["last_text"], "hello there");
        assert_eq!(e[0]["duration_ms"], 1234);
        assert_eq!(e[0]["aborted"], false);
        assert_eq!(e[0]["usage"]["input_tokens"], 20);
        assert_eq!(e[0]["usage"]["output_tokens"], 6);
        assert_eq!(e[0]["api_cost"], 1.0);
        assert_eq!(e[0]["background_tasks"][0]["id"], "b1");
        assert!(!t.active);
    }

    /// Moved to the background while busy (agents page): the background claude finishes the reply
    /// with no UserPromptSubmit; its Stop, under another prompt id, ends the turn, which keeps the
    /// prompt id the caller knows.
    #[test]
    fn a_turn_moved_to_the_background_ends_at_the_background_claude_s_stop() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "MOCK-SLOW please");
        // Another turn's Stop is not this one's until the conversation moved.
        assert!(stop(&mut t, now, "bg-1", "other").is_empty());
        t.moved();
        assert!(matches!(
            stop(&mut t, now, "bg-1", "slow slow")[..],
            [TurnOut::Stopped(_)]
        ));
        let out = marker(&mut t, now, 8000);
        let e = ends(&out);
        assert_eq!(e[0]["prompt_id"], "p1");
        assert_eq!(e[0]["last_text"], "slow slow");
        assert_eq!(e[0]["aborted"], false);
        assert!(!t.active);
    }

    #[test]
    fn a_turn_moved_to_an_idle_background_claude_ends_as_aborted() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "hi");
        t.moved();
        // A claimed spare reads idle for a while before it takes the reply up again.
        assert!(t.poll(now + ABORT_GRACE, Some(now)).is_empty());
        let out = t.poll(now + MOVED_GRACE, Some(now));
        assert_eq!(ends(&out)[0]["aborted"], true);
        assert_eq!(ends(&out)[0]["prompt_id"], "p1");
        // Idle at the move: nothing to carry.
        t.moved();
        assert!(!t.active);
    }

    /// Input typed into the background claude while it finishes the moved turn carries its own
    /// prompt id: a steer of that turn, not a new prompt ending it as aborted.
    #[test]
    fn a_steer_during_a_moved_turn_keeps_it_open() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "MOCK-SLOW please");
        t.moved();
        assert!(submit(&mut t, now, "bg-1", "and also this").is_empty());
        assert!(t.active);
        assert!(matches!(
            stop(&mut t, now, "bg-1", "done")[..],
            [TurnOut::Stopped(_)]
        ));
        let out = marker(&mut t, now, 100);
        let e = ends(&out);
        assert_eq!(e[0]["prompt_id"], "p1");
        assert_eq!(e[0]["aborted"], false);
    }

    /// A prompt (a person's, a task notification) while the background claude sits idle with
    /// the moved turn still open is a new turn: the moved one ends as aborted.
    #[test]
    fn a_prompt_while_the_background_claude_is_idle_is_a_new_turn() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "hi");
        t.moved();
        let m = HookMsg {
            event: "UserPromptSubmit".into(),
            input: json!({"prompt_id": "bg-2", "prompt": "next"}),
            reply: None,
        };
        let out = t.on_hook(&m, now, false);
        let e = ends(&out);
        assert_eq!(e[0]["prompt_id"], "p1");
        assert_eq!(e[0]["aborted"], true);
        assert_eq!(out.last(), Some(&TurnOut::Opened));
        stop(&mut t, now, "bg-2", "done");
        let out = marker(&mut t, now, 100);
        assert_eq!(ends(&out)[0]["prompt_id"], "bg-2");
    }

    #[test]
    fn a_stop_whose_marker_never_comes_ends_after_the_catch_up() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "hi");
        stop(&mut t, now, "p1", "done");
        assert!(t.poll(now + STOP_CATCH_UP / 2, None).is_empty());
        let out = t.poll(now + STOP_CATCH_UP, None);
        assert_eq!(ends(&out)[0]["last_text"], "done");
        assert!(!t.active);
    }

    /// The steer typed after the turn's last tool: claude fires both Stops before writing either
    /// turn; each turn still ends with its own prompt id, the steer's opened without a
    /// UserPromptSubmit of its own.
    #[test]
    fn a_steer_not_delivered_within_the_turn_is_a_turn_of_its_own_after_stop() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "MOCK-SLOW please");
        // Input while busy carries the running turn's prompt id: no new turn.
        assert!(submit(&mut t, now, "p1", "MOCK-STEERED now").is_empty());
        stop(&mut t, now, "p1", "slow");
        // The steer's Stop, before the transcript wrote the first turn: deferred.
        assert!(stop(&mut t, now, "p2", "STEER-TURN").is_empty());
        let out = marker(&mut t, now, 8000);
        assert_eq!(out.len(), 3, "{out:?}");
        assert_eq!(ends(&out)[0]["prompt_id"], "p1");
        assert_eq!(out[1], TurnOut::Opened);
        assert!(matches!(out[2], TurnOut::Stopped(_)));
        assert!(t.active);
        let out = marker(&mut t, now, 100);
        let e = ends(&out);
        assert_eq!(e[0]["prompt_id"], "p2");
        assert_eq!(e[0]["last_text"], "STEER-TURN");
        assert_eq!(e[0]["aborted"], false);
        assert!(!t.active);
    }

    #[test]
    fn a_steer_delivered_within_the_turn_opens_no_turn() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "go");
        submit(&mut t, now, "p1", "also this");
        t.on_tx(&block(BlockKind::User, "also this ", true), now);
        stop(&mut t, now, "p1", "ok");
        let out = marker(&mut t, now, 10);
        assert_eq!(ends(&out).len(), 1);
        assert!(!out.contains(&TurnOut::Opened));
        assert!(!t.active);
    }

    #[test]
    fn a_bang_command_runs_as_a_turn_ended_by_its_stop() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        let bang = TxEvent::UserPrompt {
            uuid: "u1".into(),
            text: "! echo hi".into(),
            images: 0,
        };
        assert_eq!(t.on_tx(&bang, now), vec![TurnOut::Opened]);
        assert!(t.active);
        stop(&mut t, now, "b1", "ok");
        let out = t.poll(now + STOP_CATCH_UP, None);
        assert_eq!(ends(&out).len(), 1);
        assert!(!t.active);
        // A slash command opens nothing.
        let slash = TxEvent::UserPrompt {
            uuid: "u2".into(),
            text: "/model x".into(),
            images: 0,
        };
        assert!(t.on_tx(&slash, now).is_empty());
        assert!(!t.active);
    }

    #[test]
    fn a_provisional_turn_that_never_runs_is_dropped_without_turn_end() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "go");
        submit(&mut t, now, "p1", "steer");
        stop(&mut t, now, "p1", "ok");
        // The steer's turn opens at the first turn's end, while the Session is idle: its idle
        // counts from then.
        let idle = now;
        let at = now + STOP_CATCH_UP;
        let out = t.poll(at, Some(idle));
        assert_eq!(ends(&out).len(), 1);
        assert!(out.contains(&TurnOut::Opened));
        assert!(t.active);
        assert!(t.poll(at + ABORT_GRACE / 2, Some(at)).is_empty());
        assert!(t.poll(at + ABORT_GRACE, Some(at)).is_empty());
        assert!(!t.active);
    }

    #[test]
    fn a_task_notification_runs_as_its_own_turn_after_the_prompts() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "MOCK-BG go");
        stop(&mut t, now, "p1", "started");
        // Right after Stop the notification's turn begins (before the marker came): the first
        // turn ends first, and a follow-up must wait (a turn is open) until the notification's
        // own Stop.
        let out = submit(
            &mut t,
            now,
            "p2",
            "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>",
        );
        assert_eq!(ends(&out)[0]["prompt_id"], "p1");
        assert_eq!(out.last(), Some(&TurnOut::Opened));
        assert!(t.active);
        stop(&mut t, now, "p2", "noted");
        let out = marker(&mut t, now, 5);
        let e = ends(&out);
        assert_eq!(e[0]["trigger"], "task_notification");
        assert_eq!(e[0]["prompt_id"], "p2");
        assert!(!t.active);
    }

    #[test]
    fn an_api_error_ends_the_turn_with_it() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "MOCK-FAIL");
        let out = hook(
            &mut t,
            now,
            "StopFailure",
            json!({"prompt_id": "p1", "error": "invalid_request", "last_assistant_message": "API Error: 400"}),
        );
        assert!(matches!(out[..], [TurnOut::Stopped(_)]));
        let e = t.poll(now + STOP_CATCH_UP, None);
        let e = ends(&e);
        assert_eq!(
            e[0]["error"],
            json!({"kind": "invalid_request", "message": "API Error: 400"})
        );
        assert_eq!(e[0]["last_text"], Value::Null);
        assert_eq!(e[0]["aborted"], false);
        // The transcript's record of an API error counts too, once.
        submit(&mut t, now, "p2", "again");
        let api = |m: &str| TxEvent::ApiError {
            kind: Some("server_error".into()),
            message: m.into(),
        };
        t.on_tx(&api("first"), now);
        t.on_tx(&api("second"), now);
        stop(&mut t, now, "p2", "x");
        let out = marker(&mut t, now, 1);
        assert_eq!(ends(&out)[0]["error"]["message"], "first");
    }

    #[test]
    fn the_input_box_shows_the_prompt_whole_or_a_long_ones_start() {
        assert!(shows_prompt("改成两行 的那个", "改成两行\n的那个"));
        let long = "a".repeat(30);
        assert!(shows_prompt(&long[..20], &long));
        assert!(!shows_prompt(&long[..10], &long));
        assert!(!shows_prompt("", "x"));
        assert!(!shows_prompt("something else", "the prompt"));
    }

    #[test]
    fn a_turn_that_goes_idle_without_stop_ends_aborted() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "go");
        t.on_tx(&block(BlockKind::Text, "partial", false), now);
        assert!(t.poll(now + ABORT_GRACE, None).is_empty(), "not idle");
        assert!(t.poll(now + ABORT_GRACE / 2, Some(now)).is_empty());
        let out = t.poll(now + ABORT_GRACE, Some(now));
        let e = ends(&out);
        assert_eq!(e[0]["aborted"], true);
        assert_eq!(e[0]["last_text"], Value::Null);
        assert!(!t.active);
        // A new prompt while the previous turn never got its Stop ends that one first.
        submit(&mut t, now, "p2", "one");
        let out = submit(&mut t, now, "p3", "two");
        assert_eq!(ends(&out)[0]["prompt_id"], "p2");
        assert_eq!(ends(&out)[0]["aborted"], true);
        assert_eq!(out.last(), Some(&TurnOut::Opened));
    }

    #[test]
    fn a_turn_running_at_connect_is_ended_by_its_next_stop() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        // A `!` command's Stop with no turn open: nothing.
        assert!(stop(&mut t, now, "bang", "").is_empty());
        t.adopt_running();
        // A prompt during it is a steer of it.
        assert!(submit(&mut t, now, "px", "more").is_empty());
        t.on_tx(&block(BlockKind::User, "more", true), now);
        stop(&mut t, now, "px", "fine");
        let out = marker(&mut t, now, 1);
        assert_eq!(ends(&out)[0]["prompt_id"], "px");
        assert!(!out.contains(&TurnOut::Opened));
    }

    #[test]
    fn send_now_ends_the_turn_aborted_and_runs_the_queue_as_the_next() {
        let (mut t, now) = (TurnTracker::new(), Instant::now());
        submit(&mut t, now, "p1", "MOCK-SLOW please");
        submit(&mut t, now, "p1", "MOCK-STEERED now");
        let out = t.send_now();
        assert_eq!(ends(&out)[0]["aborted"], true);
        assert_eq!(ends(&out)[0]["prompt_id"], "p1");
        // The queue's turn learns its prompt id from its first tool call (or its Stop).
        hook(
            &mut t,
            now,
            "PreToolUse",
            json!({"prompt_id": "p2", "tool_use_id": "t1"}),
        );
        stop(&mut t, now, "p2", "STEER-TURN");
        let out = marker(&mut t, now, 1);
        assert_eq!(ends(&out)[0]["prompt_id"], "p2");
        assert_eq!(ends(&out)[0]["aborted"], false);
    }

    #[test]
    fn a_slash_command_confirms_whatever_spaces_were_typed() {
        let mut m = Marks::default();
        m.prompt("u1".into(), "/release v1".into(), 0);
        assert_eq!(m.prompt_after(0, "/release  v1 "), Some("u1"));
        assert_eq!(m.prompt_after(1, "/release v1"), None);
        m.prompt("u2".into(), "hello world".into(), 0);
        assert_eq!(m.prompt_after(0, "hello  world"), None);
    }

    #[test]
    fn a_plugin_command_typed_by_its_bare_name_confirms() {
        let mut m = Marks::default();
        // As recorded from `<command-name>/oh-my-claudecode:autopilot</command-name>`.
        m.prompt(
            "u1".into(),
            "/oh-my-claudecode:autopilot 支持按 UP\n继续编辑".into(),
            0,
        );
        assert_eq!(
            m.prompt_after(0, "/autopilot 支持按 UP\n继续编辑"),
            Some("u1")
        );
        // The full name still matches; other names or arguments do not.
        assert_eq!(
            m.prompt_after(0, "/oh-my-claudecode:autopilot 支持按 UP 继续编辑"),
            Some("u1")
        );
        assert_eq!(m.prompt_after(0, "/pilot 支持按 UP 继续编辑"), None);
        assert_eq!(
            m.prompt_after(0, "/other:autopilot 支持按 UP 继续编辑"),
            None
        );
        assert_eq!(m.prompt_after(0, "/autopilot 别的"), None);
        assert_eq!(m.prompt_after(0, "autopilot 支持按 UP 继续编辑"), None);
    }

    #[test]
    fn marks_compare_by_sequence_not_length() {
        let mut m = Marks::default();
        for i in 0..100 {
            m.prompt(format!("u{i}"), format!("p{i}"), 0);
        }
        let before = m.seq;
        assert!(
            m.prompt_after(before, "p99").is_none(),
            "old prompts do not count"
        );
        m.prompt("new".into(), "hello ".into(), 0);
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

    /// Marks fed from a transcript written as Claude Code records it.
    fn marks_from(lines: &[Value]) -> Marks {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&main, text).unwrap();
        let mut m = Marks::default();
        for e in crate::transcript::Tailer::new(&main).poll() {
            match e {
                TxEvent::UserPrompt { uuid, text, images } => m.prompt(uuid, text, images),
                TxEvent::Enqueued { content } => m.enqueue(content),
                _ => {}
            }
        }
        m
    }

    fn image() -> Value {
        json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}})
    }

    fn parts(items: &[Option<&str>]) -> Vec<rpc::Part> {
        items
            .iter()
            .map(|p| match p {
                Some(t) => rpc::Part::Text(t.to_string()),
                None => rpc::Part::Image("/tmp/x.png".into()),
            })
            .collect()
    }

    #[test]
    fn interleaved_parts_confirm_by_text_and_image_count() {
        // Pasted in order, Claude Code records one text with the placeholders in place, then the
        // images (2.1.318).
        let m = marks_from(&[
            json!({"type": "user", "uuid": "u1", "imagePasteIds": [3, 5],
            "message": {"role": "user", "content": [
                {"type": "text", "text": "AAA [Image #3] BBB [Image #5] CCC"}, image(), image(),
            ]}}),
        ]);
        let typed = Typed::parts(&parts(&[
            Some("AAA "),
            None,
            Some(" BBB "),
            None,
            Some(" CCC"),
        ]));
        assert_eq!((typed.text.as_str(), typed.images), ("AAA  BBB  CCC", 2));
        assert_eq!(m.typed_after(0, &typed), Some("u1"));
        assert_eq!(m.typed_after(1, &typed), None, "only after the mark");
        // Another image count, or other text, is another prompt.
        let one = Typed::parts(&parts(&[Some("AAA "), None, Some(" BBB  CCC")]));
        assert_eq!(m.typed_after(0, &one), None, "the record has 2 images");
        let other = Typed::parts(&parts(&[Some("AAA "), None, Some(" BBB "), None]));
        assert_eq!(m.typed_after(0, &other), None);
        // The plugin reports the same text.
        assert!(typed.hook_matches("AAA [Image #3] BBB [Image #5] CCC"));
        assert!(!typed.hook_matches("AAA [Image #3] BBB"));
        // As the old form would look for it: not found (its text is not at the end alone).
        assert_eq!(
            m.typed_after(0, &Typed::plain("AAA  BBB  CCC".into(), 2)),
            None
        );
    }

    #[test]
    fn interleaved_parts_confirm_with_long_pastes_set_apart() {
        // An account that wraps long pastes in `<pasted_content>` sets them apart with blank
        // lines (seen after a leading image, 2.1.317): whitespace is not compared.
        let m = marks_from(&[json!({"type": "user", "uuid": "u1",
        "message": {"role": "user", "content": [
            {"type": "text", "text": "look[Image #1]\n\n<pasted_content id=\"1b44\">\nline1\nline2\n</pasted_content id=\"1b44\">\n[Image #2]"},
            image(), image(),
        ]}})]);
        let typed = Typed::parts(&parts(&[Some("look"), None, Some("line1\nline2"), None]));
        assert_eq!(m.typed_after(0, &typed), Some("u1"));
        // Images only.
        let m = marks_from(&[json!({"type": "user", "uuid": "u2",
            "message": {"role": "user", "content": [{"type": "text", "text": "[Image #7]"}, image()]}})]);
        let only = Typed::parts(&parts(&[None]));
        assert_eq!((only.text.as_str(), only.needle.as_str()), ("", "[Image #"));
        assert_eq!(m.typed_after(0, &only), Some("u2"));
    }

    #[test]
    fn interleaved_steer_confirms_by_its_enqueue() {
        // Busy, the queue holds the interleaved text with the placeholders.
        let m = marks_from(&[
            json!({"type": "queue-operation", "operation": "enqueue", "content": "see [Image #2] and [Image #4] now"}),
        ]);
        let typed = Typed::parts(&parts(&[
            Some("see "),
            None,
            Some(" and "),
            None,
            Some(" now"),
        ]));
        assert!(m.typed_enqueued_after(0, &typed));
        assert!(!m.typed_enqueued_after(1, &typed));
        // One placeholder short.
        let three = Typed::parts(&parts(&[
            Some("see "),
            None,
            Some(" and "),
            None,
            Some(" now"),
            None,
        ]));
        assert!(!m.typed_enqueued_after(0, &three));
        // A literal `[Image #9]` typed in the text is not one of the images.
        let m = marks_from(&[
            json!({"type": "queue-operation", "operation": "enqueue", "content": "a [Image #9] b[Image #3]"}),
        ]);
        let literal = Typed::parts(&parts(&[Some("a [Image #9] b"), None]));
        assert!(m.typed_enqueued_after(0, &literal));
        // The old form keeps its own rule: the text after the leading placeholders.
        let m = marks_from(&[
            json!({"type": "queue-operation", "operation": "enqueue", "content": "[Image #1] look"}),
        ]);
        assert!(m.typed_enqueued_after(0, &Typed::plain("look".into(), 1)));
        assert!(!m.typed_enqueued_after(0, &Typed::plain("look".into(), 0)));
    }
}
