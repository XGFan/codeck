//! What goes out for the current waiting: a permission request, a question, a choice from the
//! layout library (or the folder trust dialog), the analyzer's result, or the dialog's screen.

use super::*;

/// Whether a choice layout may be announced while tools (`tools`: the open requests) wait for
/// permission. Claude Code's own approvals are numbered and answered as permission requests, so
/// that one announced as a choice would hide the tool from the caller; only a tool whose approval
/// is a menu (ExitPlanMode) is announced that way. A menu without numbers is the tool's own, drawn
/// with no approval behind it (computer use).
fn choice_during_permission(select: Select, tools: &[&str]) -> bool {
    select == Select::Cursor
        || (!tools.is_empty() && tools.iter().all(|t| CHOICE_TOOLS.contains(t)))
}

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

/// What went out for the current waiting.
#[derive(Debug)]
pub(super) enum Announced {
    /// A permission request or a question: its tool_use id.
    Request(String),
    /// A choice: its request id (`choice:N`), its layout, the event sent, and since when the
    /// layout has been off the screen.
    Choice {
        id: String,
        layout: String,
        event: Value,
        gone_since: Option<Instant>,
    },
    /// A dialog handed to the caller as its screen: the Process File's `waitingFor`.
    Dialog(String),
}

impl Announced {
    /// Answered by `request_id` (a dialog has no request id).
    pub(super) fn is(&self, request_id: &str) -> bool {
        match self {
            Announced::Request(id) | Announced::Choice { id, .. } => id == request_id,
            Announced::Dialog(_) => false,
        }
    }

    /// The folder trust dialog, announced as a choice.
    pub(super) fn is_trust(&self) -> bool {
        matches!(self, Announced::Choice { layout, .. } if layout == TRUST_LAYOUT)
    }
}

impl Engine {
    /// Startup screens (onboarding, folder trust) come before the Process File: report each
    /// distinct one, checking at most once a second.
    pub(super) fn report_startup_screen(&mut self) {
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
        // The folder trust dialog is answered as a choice.
        if recognize::trust_folder(&lines).is_some() {
            if !self.announced.as_ref().is_some_and(Announced::is_trust) {
                self.choice_seq += 1;
                let rid = format!("choice:{}", self.choice_seq);
                let ev = json!({"type": "choice", "request_id": rid, "title": recognize::TRUST_TITLE,
                    "options": [
                        {"label": recognize::TRUST_YES, "meaning": "allow", "description": null},
                        {"label": recognize::TRUST_NO, "meaning": "deny", "description": null}],
                    "tool_use_id": null, "layout": TRUST_LAYOUT, "layout_source": "builtin"});
                crate::clog!("recognize choice layout={TRUST_LAYOUT} source=builtin");
                crate::log::screen(lines.iter().map(String::as_str));
                self.announced = Some(Announced::Choice {
                    id: rid,
                    layout: TRUST_LAYOUT.to_string(),
                    event: ev.clone(),
                    gone_since: None,
                });
                self.startup_last = Some(lines);
                self.emit(ev);
            }
            return;
        }
        if self.announced.as_ref().is_some_and(Announced::is_trust) {
            self.announced = None;
        }
        if self.startup_last.as_ref() != Some(&lines) {
            self.startup_last = Some(lines.clone());
            self.emit(json!({"type": "dialog", "waiting_for": "startup", "lines": lines}));
        }
    }

    /// Answers the folder trust dialog: the cursor onto the option, then Enter.
    pub(super) fn answer_trust(&mut self, option: &str) -> CmdResult {
        let want_yes = if recognize::TRUST_YES.starts_with(option) && !option.is_empty() {
            true
        } else if recognize::TRUST_NO.starts_with(option) && !option.is_empty() {
            false
        } else {
            return err(
                "bad_request",
                format!(
                    "没有对应的选项：{option}；可选 [{:?}, {:?}]",
                    recognize::TRUST_YES,
                    recognize::TRUST_NO
                ),
            );
        };
        for _ in 0..3 {
            let now = self.fresh_screen()?;
            let Some(on_yes) = recognize::trust_folder(&now.lines) else {
                return Err(self.unrecognized("answer_choice", Some(&now)));
            };
            if on_yes == want_yes {
                self.keys(key::ENTER)?;
                let gone = self.wait_screen(Duration::from_secs(10), |_, s| {
                    recognize::trust_folder(&s.lines).is_none().then_some(())
                });
                if gone.is_none() {
                    return err("timeout", "回车后 10 秒内信任框没有关闭");
                }
                self.announced = None;
                return Ok(json!({"state": self.state.name()}));
            }
            self.keys(if want_yes { key::DOWN } else { key::UP })?;
            self.wait_screen(Duration::from_millis(500), |_, s| {
                (recognize::trust_folder(&s.lines) == Some(want_yes)).then_some(())
            });
        }
        Err(self.unrecognized("answer_choice", None))
    }

    /// An announced request whose tool got its result is over even if the Process File still says
    /// waiting (two prompts in a row can collapse into one waiting period): announce afresh.
    pub(super) fn refresh_announcement(&mut self) {
        let Some(a) = &self.announced else {
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
        match a {
            Announced::Dialog(_) => {}
            // A choice is over once its tool got a result, or its layout has been off the screen
            // a moment (two waits can merge into one waiting period): announce afresh.
            Announced::Choice { layout, event, .. } => {
                let tool_done = event["tool_use_id"]
                    .as_str()
                    .is_some_and(|tid| self.resolved.contains(tid));
                let shown = self
                    .screen()
                    .ok()
                    .and_then(|s| {
                        self.library
                            .find_kind(&s.lines, LayoutKind::Choice)
                            .map(|(l, _)| &l.id == layout)
                    })
                    .unwrap_or(false);
                let mut gone = false;
                if let Some(Announced::Choice { gone_since, .. }) = &mut self.announced {
                    if shown {
                        *gone_since = None;
                    } else if gone_since.is_none() {
                        *gone_since = Some(Instant::now());
                    }
                    gone = gone_since.is_some_and(|t| t.elapsed() >= Duration::from_millis(250));
                }
                if tool_done || gone {
                    self.announced = None;
                    self.raw_since = Some((
                        self.raw_since.take().map(|r| r.0).unwrap_or_default(),
                        Instant::now(),
                    ));
                }
            }
            // A question answered on the screen has no hook: it is open while its tool is pending.
            Announced::Request(id) => {
                let screen_ask = self.pending_ask().is_some_and(|p| p.id == *id);
                if self.open_request(id).is_none() && !screen_ask {
                    if let Some(Announced::Request(done)) = self.announced.take() {
                        self.resolved.insert(done);
                    }
                    self.raw_since = Some((
                        self.raw_since.take().map(|r| r.0).unwrap_or_default(),
                        Instant::now(),
                    ));
                }
            }
        }
    }

    pub(super) fn waiting_age(&self) -> Duration {
        self.raw_since
            .as_ref()
            .map(|(_, t)| t.elapsed())
            .unwrap_or_default()
    }

    pub(super) fn announce_requests(&mut self) {
        self.poll_analysis();
        let State::Waiting { kind, waiting_for } = self.state.clone() else {
            return;
        };
        // The Rewind picker and confirm page are a rewind's own, not a dialog to report.
        if self.rewind.is_some() {
            return;
        }
        // Just switched to WORK_SIZE: Claude Code is redrawing what it showed.
        if self.announced.is_some() || self.answered_until.is_some() || self.growing() {
            return;
        }
        match kind {
            WaitKind::Permission => {
                // A main-line request waits in its hook: answered through it, whatever the screen.
                // Its dialog is drawn meanwhile: announced once the options on it are read (for
                // "always allow"), or without them after OPTIONS_GRACE.
                let screen = self.screen().ok();
                if let Some(r) = self
                    .requests
                    .iter()
                    .find(|r| r.is_approval() && r.reply.is_some())
                {
                    let opts = screen
                        .as_ref()
                        .and_then(|s| self.dialog_options(r, &s.lines));
                    if opts.is_none() && r.at.elapsed() < OPTIONS_GRACE {
                        return;
                    }
                    // The options cannot be read at this size: read them at WORK_SIZE.
                    if opts.is_none() && self.grow() {
                        return;
                    }
                    crate::clog!(
                        "recognize permission {} options={}",
                        r.id,
                        opts.as_ref().map_or("none".into(), |(_, src)| src.clone())
                    );
                    if let Some(s) = &screen {
                        crate::log::screen(s.lines.iter().map(String::as_str));
                    }
                    let ev = Self::permission_event(r, opts);
                    self.announced = Some(Announced::Request(r.id.clone()));
                    self.emit(ev);
                    return;
                }
                if let Some(s) = &screen {
                    self.adopt_pending(s);
                }
                if let Some(id) = screen.as_ref().and_then(|s| self.screen_request(s))
                    && let Some(r) = self.open_request(&id)
                {
                    let opts = screen
                        .as_ref()
                        .and_then(|s| self.dialog_options(r, &s.lines));
                    crate::clog!("recognize permission {} on screen", r.id);
                    let ev = Self::permission_event(r, opts);
                    self.announced = Some(Announced::Request(id));
                    self.emit(ev);
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
                if self.waiting_age() >= MATCH_GRACE {
                    // Known layouts may only be cut off: look again at WORK_SIZE first.
                    if self.grow() || self.analyzing(&s, &waiting_for) {
                        return;
                    }
                    // Cannot tie the dialog to a request: hand the screen to the caller.
                    self.announced = Some(Announced::Dialog(waiting_for.clone()));
                    self.emit(
                        json!({"type": "dialog", "waiting_for": waiting_for, "lines": s.lines}),
                    );
                }
            }
            WaitKind::Question => {
                let (id, input, agent_id) = match self
                    .requests
                    .iter()
                    .find(|r| r.tool_name == "AskUserQuestion")
                {
                    Some(r) => (r.id.clone(), r.input.clone(), r.agent_id.clone()),
                    None => match self.pending_ask() {
                        Some(p) => (p.id.clone(), p.input.clone(), None),
                        None => return,
                    },
                };
                let ev = json!({"type": "question", "request_id": id,
                    "questions": input.get("questions").cloned().unwrap_or(Value::Null), "agent_id": agent_id});
                self.announced = Some(Announced::Request(id));
                self.emit(ev);
            }
            WaitKind::Dialog => {
                // The Process File says waiting before the dialog is drawn: let it render first.
                if self.waiting_age() < Duration::from_millis(250) {
                    return;
                }
                let s = self.screen().unwrap_or_default();
                if self.announce_choice(&s) || self.grow() || self.analyzing(&s, &waiting_for) {
                    return;
                }
                self.announced = Some(Announced::Dialog(waiting_for.clone()));
                self.emit(json!({"type": "dialog", "waiting_for": waiting_for, "lines": s.lines}));
            }
        }
    }

    /// A choice layout on screen: announce it. True when announced.
    pub(super) fn announce_choice(&mut self, s: &Screen) -> bool {
        let Some((l, opts)) = self.library.find_kind(&s.lines, LayoutKind::Choice) else {
            return false;
        };
        if let State::Waiting {
            kind: WaitKind::Permission,
            ..
        } = &self.state
        {
            let tools: Vec<&str> = self
                .requests
                .iter()
                .map(|r| r.tool_name.as_str())
                .filter(|t| *t != "AskUserQuestion")
                .collect();
            if !choice_during_permission(l.select, &tools) {
                return false;
            }
        }
        let (id, title, source) = (l.id.clone(), l.title.clone(), l.source.clone());
        crate::clog!("recognize choice layout={id} source={source}");
        crate::log::screen(s.lines.iter().map(String::as_str));
        // A choice raised by a tool (ExitPlanMode…): the single pending tool, if any.
        let tool = match self.requests.as_slice() {
            [r] => Some(r.id.clone()),
            _ => None,
        };
        self.choice_seq += 1;
        let rid = format!("choice:{}", self.choice_seq);
        let options: Vec<Value> = opts
            .iter()
            .map(|o| json!({"label": o.label, "meaning": o.meaning, "description": o.description}))
            .collect();
        let ev = json!({"type": "choice", "request_id": rid, "title": title, "options": options,
            "tool_use_id": tool, "layout": id, "layout_source": source});
        self.announced = Some(Announced::Choice {
            id: rid,
            layout: id,
            event: ev.clone(),
            gone_since: None,
        });
        self.emit(ev);
        true
    }

    /// Hands an unrecognized dialog to the analyzer (once per dialog, rate-limited). True while
    /// its analysis is in flight: the dialog is not reported as unrecognized meanwhile.
    pub(super) fn analyzing(&mut self, s: &Screen, waiting_for: &str) -> bool {
        // One analysis at a time; meanwhile nothing else is reported for the dialog.
        if self.analysis.is_some() {
            return true;
        }
        // What the built-in recognizers know is never sent out.
        if recognize::permission(&s.lines).is_some()
            || recognize::ask(&s.lines).is_some()
            || recognize::rewind_picker(&s.lines).is_some()
            || recognize::rewind_confirm(&s.lines).is_some()
        {
            return false;
        }
        // Learned already and not announced here: learning it again would change nothing.
        if self
            .library
            .find_kind(&s.lines, LayoutKind::Choice)
            .is_some()
        {
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
        // What the dialog may be about: the plugin's requests, else the transcript's pending tools.
        let mut pending: Vec<(String, Value)> = self
            .requests
            .iter()
            .map(|r| (r.tool_name.clone(), r.input.clone()))
            .collect();
        if pending.is_empty() {
            pending = self
                .tailer
                .as_ref()
                .map(|t| {
                    t.pending()
                        .into_iter()
                        .filter(|p| !self.resolved.contains(&p.id))
                        .filter(|p| !NO_PERMISSION_TOOLS.contains(&p.name.as_str()))
                        .map(|p| (p.name.clone(), p.input.clone()))
                        .collect()
                })
                .unwrap_or_default();
        }
        let lines = s.lines.clone();
        let waiting_for = waiting_for.to_string();
        let version = self.pf.as_ref().and_then(|p| p.version.clone());
        let session_id = self.session_id.clone();
        let (tx, rx) = mpsc::channel();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let req = analyze::Request {
                lines: &lines,
                waiting_for: &waiting_for,
                pending,
                claude_version: version,
            };
            let started = Instant::now();
            let reply = analyzer.ask(&req);
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let result = reply
                .as_ref()
                .map_err(|e| format!("{e:#}"))
                .and_then(|t| analyze::layout_from(&req, t).map_err(|e| format!("{e:#}")));
            let made = result.as_ref().ok();
            let record = Record {
                at: analyze::now_rfc3339(),
                session_id,
                claude_version: req.claude_version.clone(),
                model: analyzer.model.clone(),
                base_url: analyzer.base_url.clone(),
                elapsed_ms,
                waiting_for: waiting_for.clone(),
                pending: analyze::pending_sent(&req),
                screen: lines.clone(),
                reply: reply.ok(),
                outcome: Outcome::Failed,
                layout: made.map(|l| l.id.clone()),
                kind: made.map(|l| l.kind),
                title: made.map(|l| l.title.clone()),
                path: None,
                error: None,
            };
            let _ = tx.send((result, record));
            wake.bump();
        });
        self.analysis = Some((sig, rx));
        true
    }

    /// Takes the analyzer's answer: a layout that passes its structural checks on the screen it
    /// came from goes into the library (and is used at once, ADR-0002).
    pub(super) fn poll_analysis(&mut self) {
        let Some((_, rx)) = &self.analysis else {
            return;
        };
        let res = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.analysis = None;
                self.emit(json!({"type": "analysis_failed", "error": "分析器线程意外结束"}));
                return;
            }
        };
        self.analysis = None;
        let (res, mut record) = res;
        let res = match res {
            Ok(l) if l.match_screen(&l.screen).is_some() => {
                let (id, kind, title) = (l.id.clone(), l.kind, l.title.clone());
                self.library
                    .save(l)
                    .map(|path| (id, kind, title, path))
                    .map_err(|e| (format!("{e:#}"), None))
            }
            Ok(l) => Err(("分析结果与屏幕对不上".to_string(), Some(l.title))),
            Err(e) => Err((e, None)),
        };
        match &res {
            Ok((_, _, _, path)) => {
                record.outcome = Outcome::Learned;
                record.path = Some(path.display().to_string());
            }
            Err((e, _)) => record.error = Some(e.clone()),
        }
        if let Err(e) = analysis_records::write(&analysis_records::dir(), &record) {
            crate::clog!("analysis record: {e:#}");
        }
        match res {
            Ok((id, kind, title, path)) => self
                .emit(json!({"type": "layout_learned", "layout": id,
                "kind": kind, "title": title, "path": path.display().to_string()})),
            Err((e, Some(title))) => {
                self.emit(json!({"type": "analysis_failed", "error": e, "title": title}))
            }
            Err((e, None)) => self.emit(json!({"type": "analysis_failed", "error": e})),
        }
    }

    /// Waits until the waiting dialog has been up [`DIALOG_SETTLE`].
    pub(super) fn settle_dialog(&mut self) {
        let left = DIALOG_SETTLE.saturating_sub(self.waiting_age());
        if !left.is_zero() {
            self.wait_until(left, |_| false);
        }
    }

    /// Answers the announced choice by an option's text (exact, else a unique prefix).
    pub(super) fn answer_choice(&mut self, request_id: &str, option: &str) -> CmdResult {
        if self
            .announced
            .as_ref()
            .is_some_and(|a| a.is_trust() && a.is(request_id))
        {
            return self.answer_trust(option);
        }
        match &self.state {
            State::Waiting { .. } => {}
            State::Exited => return err("exited", "Session 已结束"),
            _ => return err("not_waiting", format!("当前状态为 {}", self.state.name())),
        }
        // The layout of the choice being answered: the key goes only to a screen showing it.
        let layout = match &self.announced {
            Some(Announced::Choice { id, layout, .. }) if id == request_id => layout.clone(),
            _ => {
                return err(
                    "unknown_request",
                    format!("当前挂起的选择不是 {request_id}"),
                );
            }
        };
        self.settle_dialog();
        let now = self.fresh_screen()?;
        // The wait ticked: the choice may be over, or a newer one announced meanwhile.
        if !self.announced.as_ref().is_some_and(|a| a.is(request_id)) {
            return err(
                "unknown_request",
                format!("作答前选择已经变了，不再是 {request_id}"),
            );
        }
        let Some((l, opts)) = self.library.find_kind(&now.lines, LayoutKind::Choice) else {
            return Err(self.unrecognized("answer_choice", Some(&now)));
        };
        if l.id != layout {
            return err("unknown_request", "屏幕上已经不是这个选择");
        }
        let select = l.select;
        let exact: Vec<_> = opts.iter().filter(|o| o.label == option).collect();
        let prefix: Vec<_> = opts
            .iter()
            .filter(|o| o.label.starts_with(option))
            .collect();
        let pick = match (exact.as_slice(), prefix.as_slice()) {
            ([o], _) | ([], [o]) => o.number,
            _ => {
                let labels: Vec<&str> = opts.iter().map(|o| o.label.as_str()).collect();
                return err(
                    "bad_request",
                    format!("没有唯一对应的选项：{option}；可选 {labels:?}"),
                );
            }
        };
        // Where `❯` is while the screen still shows this choice.
        let cursor_of = |e: &Engine, s: &Screen| -> Option<u32> {
            let (l, opts) = e.library.find_kind(&s.lines, LayoutKind::Choice)?;
            if l.id != layout {
                return None;
            }
            opts.iter().find(|o| o.cursor).map(|o| o.number)
        };
        self.press_option(select, pick, &cursor_of)?;
        let rid = request_id.to_string();
        let ok = self.wait_until(Duration::from_secs(10), |e| {
            !matches!(e.state, State::Waiting { .. })
                || !e.announced.as_ref().is_some_and(|a| a.is(&rid))
        });
        if ok {
            Ok(json!({"state": self.state.name()}))
        } else {
            err("timeout", "作答后 10 秒内状态没有变化")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_while_a_tool_waits_for_permission() {
        // The tool's own menu, whether or not its call could be tied to the screen.
        assert!(choice_during_permission(Select::Cursor, &[]));
        assert!(choice_during_permission(Select::Cursor, &["Bash"]));
        // A numbered menu: only ExitPlanMode's.
        assert!(choice_during_permission(Select::Number, &["ExitPlanMode"]));
        assert!(!choice_during_permission(Select::Number, &["Bash"]));
        assert!(!choice_during_permission(
            Select::Number,
            &["ExitPlanMode", "Bash"]
        ));
        assert!(!choice_during_permission(Select::Number, &[]));
    }
}
