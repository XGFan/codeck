//! Permission requests and questions: paired from the plugin's hooks (or the transcript's pending
//! tools), tied to the dialog on screen, and answered through the waiting hook or with keys.

use super::*;

/// Text to look for in a permission dialog's body for this tool (whitespace removed, first 40
/// chars), or `None` when the tool's input gives nothing recognizable.
fn dialog_needle(name: &str, input: &Value) -> Option<String> {
    let field = match name {
        "Bash" => "command",
        "Edit" | "Write" | "MultiEdit" | "Read" | "NotebookEdit" => "file_path",
        "WebFetch" => "url",
        "Glob" | "Grep" => "pattern",
        _ => return None,
    };
    let v = input.get(field)?.as_str()?;
    let v = if field == "file_path" {
        v.rsplit('/').next().unwrap_or(v)
    } else {
        v
    };
    let n: String = v.chars().filter(|c| !c.is_whitespace()).take(40).collect();
    (!n.is_empty()).then_some(n)
}

/// AskUserQuestion's `answers` (question text → answer) for the caller's `answers`, one per
/// question: option labels (several joined by `, ` for a multi-select), or free text.
fn question_answers(
    questions: &[Value],
    answers: &[Answer],
) -> std::result::Result<serde_json::Map<String, Value>, String> {
    if questions.len() != answers.len() {
        return Err(format!(
            "有 {} 道题，给了 {} 个答案",
            questions.len(),
            answers.len()
        ));
    }
    let mut out = serde_json::Map::new();
    for (i, (q, a)) in questions.iter().zip(answers).enumerate() {
        let title = q
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let multi = q
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let labels: Vec<&str> = q
            .get("options")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|o| o.get("label").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let text = match a {
            Answer::Options(chosen) => {
                if let Some(bad) = chosen.iter().find(|c| !labels.contains(&c.as_str())) {
                    return Err(format!("第 {} 题没有选项 {bad}", i + 1));
                }
                if !multi && chosen.len() != 1 {
                    return Err(format!("第 {} 题是单选", i + 1));
                }
                if chosen.is_empty() {
                    return Err(format!("第 {} 题至少要选一项", i + 1));
                }
                chosen.join(", ")
            }
            Answer::Other(t) => {
                let t = pty::sanitize(t).replace(['\n', '\t'], " ");
                if t.trim().is_empty() {
                    return Err(format!("第 {} 题的 other 为空", i + 1));
                }
                t
            }
        };
        out.insert(title.to_string(), json!(text));
    }
    Ok(out)
}

/// A tool call the plugin reported at PreToolUse (PermissionRequest has no tool_use id).
#[derive(Debug, Clone)]
pub(super) struct PreTool {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: Value,
    pub(super) agent_id: Option<String>,
}

/// A permission request (or AskUserQuestion) the plugin reported and that is not over yet.
#[derive(Debug)]
pub(super) struct HookRequest {
    /// The tool_use id (paired from PreToolUse).
    pub(super) id: String,
    pub(super) tool_name: String,
    pub(super) input: Value,
    pub(super) agent_id: Option<String>,
    /// When the plugin reported it.
    pub(super) at: Instant,
    /// Since when the Session is no longer waiting while this hook still is.
    pub(super) away_since: Option<Instant>,
    /// The waiting hook: the answer goes back through it. `None`: answered on the screen (a
    /// background Subagent's request, ExitPlanMode, or the hook is gone).
    pub(super) reply: Option<UnixStream>,
}

impl HookRequest {
    /// An approval of a tool (AskUserQuestion is a question; ExitPlanMode is a choice).
    pub(super) fn is_approval(&self) -> bool {
        self.tool_name != "AskUserQuestion" && !CHOICE_TOOLS.contains(&self.tool_name.as_str())
    }
}

/// A prompt the plugin saw submitted: `(sequence, prompt_id, text)`.
pub(super) type HookPrompt = (u64, String, String);

impl Engine {
    /// A permission request (or AskUserQuestion) from the plugin. Held for the caller only when
    /// Claude Code shows its own dialog meanwhile (ADR-0003): a background Subagent's request is
    /// not shown until the hook returns, and ExitPlanMode cannot be allowed by a hook. Those are
    /// answered on the screen instead.
    pub(super) fn on_permission_request(&mut self, m: HookMsg) {
        let i = &m.input;
        let name = i["tool_name"].as_str().unwrap_or_default().to_string();
        let input = i["tool_input"].clone();
        let agent_id = i["agent_id"].as_str().map(str::to_string);
        let taken: HashSet<&str> = self.requests.iter().map(|r| r.id.as_str()).collect();
        let id = self
            .pre_tools
            .iter()
            .rev()
            .find(|p| {
                p.name == name
                    && p.input == input
                    && p.agent_id == agent_id
                    && !taken.contains(p.id.as_str())
                    && !self.resolved.contains(&p.id)
            })
            .map(|p| p.id.clone());
        let hold = id.is_some()
            && agent_id.is_none()
            && !CHOICE_TOOLS.contains(&name.as_str())
            && !self.taken_over;
        let mut reply = m.reply;
        if !hold && let Some(r) = reply.take() {
            let _ = hook::send_reply(r, None);
        }
        let Some(id) = id else { return };
        self.requests.push(HookRequest {
            id,
            tool_name: name,
            input,
            agent_id,
            at: Instant::now(),
            away_since: None,
            reply,
        });
    }

    /// The request is over: its hook (if still waiting) is released with no decision.
    pub(super) fn resolve(&mut self, id: &str) {
        self.requests.retain(|r| r.id != id);
        if self.resolved.len() > 4096 {
            self.resolved.clear();
        }
        self.resolved.insert(id.to_string());
        if self.announced.as_ref().is_some_and(|a| a.is(id)) {
            self.announced = None;
        }
    }

    pub(super) fn open_request(&self, id: &str) -> Option<&HookRequest> {
        self.requests.iter().find(|r| r.id == id)
    }

    /// Releases every waiting hook (someone took over: the person answers in the TUI).
    pub(super) fn release_hooks(&mut self) {
        for r in &mut self.requests {
            r.reply = None;
        }
    }

    /// The pending tool the permission dialog on `s` is about: the one whose command / file shows
    /// in the dialog body; failing that, the only candidate when no candidate has anything to show.
    /// A permission dialog on `lines`: the built-in recognizer, else a permission layout from the
    /// library. With the dialog's body for matching the pending tool.
    /// Also says where the dialog's layout comes from: `builtin` or `learned`.
    pub(super) fn dialog_of(&self, lines: &[String]) -> Option<(PermissionDialog, String, String)> {
        if let Some(d) = recognize::permission(lines) {
            return Some((d, recognize::permission_body(lines)?, "builtin".into()));
        }
        let (l, opts) = self.library.find_kind(lines, LayoutKind::Permission)?;
        let options = opts
            .into_iter()
            .map(|o| (o.number, o.label, o.meaning))
            .collect();
        Some((
            PermissionDialog { options },
            l.body(lines)?,
            l.source.clone(),
        ))
    }

    /// The options of `r`'s dialog on `lines` (`[{label, meaning}]`, labels as shown) and where
    /// its layout comes from; `None` when the screen shows no dialog that fits `r`.
    pub(super) fn dialog_options(
        &self,
        r: &HookRequest,
        lines: &[String],
    ) -> Option<(Value, String)> {
        let (d, body, source) = self.dialog_of(lines)?;
        match dialog_needle(&r.tool_name, &r.input) {
            Some(n) if !body.contains(&n) => return None,
            Some(_) => {}
            // Nothing to match the dialog against: only when no other approval could be on screen.
            None if self.requests.iter().filter(|x| x.is_approval()).count() > 1 => return None,
            None => {}
        }
        let options: Vec<Value> = d
            .options
            .iter()
            // `meaning` as the layouts write it: allow / allow_always / deny / other.
            .map(|(_, label, meaning)| json!({"label": label, "meaning": meaning}))
            .collect();
        Some((Value::Array(options), source))
    }

    /// The request the permission dialog on `s` is about, among those answered on the screen
    /// (a background Subagent's): the one whose command / file shows in the dialog body; failing
    /// that, the only candidate when no candidate has anything to show.
    pub(super) fn screen_request(&self, s: &Screen) -> Option<String> {
        let (_, body, _) = self.dialog_of(&s.lines)?;
        let cands: Vec<&HookRequest> = self
            .requests
            .iter()
            .filter(|r| r.is_approval() && r.reply.is_none())
            .collect();
        let matched: Vec<&&HookRequest> = cands
            .iter()
            .filter(|r| dialog_needle(&r.tool_name, &r.input).is_some_and(|n| body.contains(&n)))
            .collect();
        // Exactly one request may match; two that both fit the dialog are ambiguous.
        match matched.len() {
            1 => return Some(matched[0].id.clone()),
            0 => {}
            _ => return None,
        }
        let blind: Vec<&&HookRequest> = cands
            .iter()
            .filter(|r| dialog_needle(&r.tool_name, &r.input).is_none())
            .collect();
        (blind.len() == 1 && cands.len() == 1).then(|| blind[0].id.clone())
    }

    /// A permission dialog the plugin never reported (it was up before ccrpc connected): taken
    /// from the transcript's pending tool whose command / file shows in it, answered with keys.
    pub(super) fn adopt_pending(&mut self, s: &Screen) {
        if self.requests.iter().any(|r| r.is_approval())
            || self.waiting_age() < Duration::from_millis(250)
        {
            return;
        }
        let Some((_, body, _)) = self.dialog_of(&s.lines) else {
            return;
        };
        let Some(t) = &self.tailer else { return };
        let cands: Vec<_> = t
            .pending()
            .into_iter()
            .filter(|p| !NO_PERMISSION_TOOLS.contains(&p.name.as_str()))
            .filter(|p| !CHOICE_TOOLS.contains(&p.name.as_str()))
            .filter(|p| !self.resolved.contains(&p.id))
            // A tool the plugin reported at PreToolUse gets its own PermissionRequest.
            .filter(|p| !self.pre_tools.iter().any(|t| t.id == p.id))
            .collect();
        let matched: Vec<_> = cands
            .iter()
            .filter(|p| dialog_needle(&p.name, &p.input).is_some_and(|n| body.contains(&n)))
            .collect();
        let pick = match (matched.as_slice(), cands.as_slice()) {
            ([p], _) => *p,
            ([], [p]) if dialog_needle(&p.name, &p.input).is_none() => p,
            _ => return,
        };
        self.requests.push(HookRequest {
            id: pick.id.clone(),
            tool_name: pick.name.clone(),
            input: pick.input.clone(),
            agent_id: pick.agent_id.clone(),
            at: Instant::now(),
            away_since: None,
            reply: None,
        });
    }

    /// `options` / `layout_source`: the dialog on screen, when it was recognized (else null).
    pub(super) fn permission_event(r: &HookRequest, options: Option<(Value, String)>) -> Value {
        let (options, source) = match options {
            Some((o, s)) => (o, Value::String(s)),
            None => (Value::Null, Value::Null),
        };
        json!({"type": "permission_request", "request_id": r.id, "tool_name": r.tool_name,
            "input": r.input, "agent_id": r.agent_id, "options": options, "layout_source": source})
    }

    /// The main line's AskUserQuestion still waiting for its result in the transcript.
    pub(super) fn pending_ask(&self) -> Option<&transcript::PendingTool> {
        self.tailer
            .as_ref()?
            .pending()
            .into_iter()
            .find(|p| p.name == "AskUserQuestion" && p.agent_id.is_none())
    }

    pub(super) fn request_is(&self, kind: &WaitKind, request_id: &str) -> bool {
        matches!(&self.state, State::Waiting { kind: k, .. } if k == kind)
            && self.announced.as_ref().is_some_and(|a| a.is(request_id))
    }

    pub(super) fn check_request(
        &self,
        kind: WaitKind,
        request_id: &str,
    ) -> std::result::Result<(), CmdErr> {
        match &self.state {
            State::Waiting { kind: k, .. } if *k == kind => {}
            State::Exited => return err("exited", "Session 已结束"),
            _ => return err("not_waiting", format!("当前状态为 {}", self.state.name())),
        }
        if !self.announced.as_ref().is_some_and(|a| a.is(request_id)) {
            return err(
                "unknown_request",
                format!("当前挂起的请求不是 {request_id}"),
            );
        }
        Ok(())
    }

    pub(super) fn wait_leave_request(&mut self, request_id: &str) -> CmdResult {
        let rid = request_id.to_string();
        let ok = self.wait_until(Duration::from_secs(10), |e| {
            !matches!(e.state, State::Waiting { .. })
                || e.announced.as_ref().is_some_and(|a| !a.is(&rid))
        });
        if ok {
            Ok(json!({"state": self.state.name()}))
        } else {
            err("timeout", "作答后 10 秒内状态没有变化")
        }
    }

    pub(super) fn answer_permission(
        &mut self,
        request_id: &str,
        decision: &Decision,
        message: Option<&str>,
    ) -> CmdResult {
        self.check_request(WaitKind::Permission, request_id)?;
        let Some(idx) = self.requests.iter().position(|r| r.id == request_id) else {
            return err(
                "unknown_request",
                format!("当前挂起的请求不是 {request_id}"),
            );
        };
        // A hook that went away while waiting: answer on the screen instead.
        if self.requests[idx]
            .reply
            .as_ref()
            .is_some_and(hook::peer_gone)
        {
            self.requests[idx].reply = None;
        }
        // "Always allow" is the dialog's own option: the rule it adds (`python3 *`) is not the
        // suggestion the hook gets (the exact command). The held hook is let go after the key.
        if *decision == Decision::AllowAlways && self.requests[idx].reply.is_some() {
            self.writable()?;
            let held = self.requests[idx].reply.take();
            let res = self.answer_permission_on_screen(request_id, decision, None);
            if res.is_err()
                && let Some(r) = self.requests.iter_mut().find(|r| r.id == request_id)
            {
                r.reply = held;
            }
            return res;
        }
        if self.requests[idx].reply.is_some() {
            self.writable()?;
            let output = match decision {
                Decision::Allow | Decision::AllowAlways => hook::allow_output(None, None),
                Decision::Deny => hook::deny_output(message),
            };
            let stream = self.requests[idx].reply.take().expect("held");
            if hook::send_reply(stream, Some(output)).is_ok() {
                self.answered(request_id);
                return self.wait_leave_request(request_id);
            }
            // The hook was gone after all: the dialog is still there for keys.
        }
        self.answer_permission_on_screen(request_id, decision, message)
    }

    /// The request was answered through its hook: it is over, and the closing dialog is not
    /// taken for a new one.
    pub(super) fn answered(&mut self, id: &str) {
        self.resolve(id);
        // `announced` stays on the request until the state moves on.
        self.announced = Some(Announced::Request(id.to_string()));
        self.answered_until = Some(Instant::now() + ANSWER_SETTLE);
    }

    /// Answers a request whose hook is not waiting (a background Subagent's) with the dialog's
    /// option key.
    pub(super) fn answer_permission_on_screen(
        &mut self,
        request_id: &str,
        decision: &Decision,
        message: Option<&str>,
    ) -> CmdResult {
        let rid = request_id.to_string();
        // The dialog on screen must be the one for this request.
        let found = self.wait_screen(Duration::from_secs(3), |e, s| {
            let (d, _, _) = e.dialog_of(&s.lines)?;
            (e.screen_request(s)? == rid).then_some(d)
        });
        let Some((d, s)) = found else {
            return Err(self.unrecognized("answer_permission", None));
        };
        let choice = match decision {
            Decision::Allow => Meaning::Allow,
            Decision::AllowAlways => Meaning::AllowAlways,
            Decision::Deny => Meaning::Deny,
        };
        let Some(n) = d.number_for(choice) else {
            if choice == Meaning::AllowAlways {
                return err("not_offered", "这次审批没有“总是允许”选项");
            }
            return Err(self.unrecognized("answer_permission", Some(&s)));
        };
        self.settle_dialog();
        // Last check right before the key: still the same request, on a Snapshot taken now.
        let now = self.fresh_screen()?;
        if !self.request_is(&WaitKind::Permission, request_id)
            || self.screen_request(&now).is_none_or(|id| id != rid)
        {
            return err(
                "unknown_request",
                format!("作答前请求已经变了，不再是 {request_id}"),
            );
        }
        self.keys(n.to_string().as_bytes())?;
        // A key the dialog dropped leaves it there: press once more if it still is this request's.
        let left = |e: &Engine| !e.request_is(&WaitKind::Permission, &rid);
        if !self.wait_until(Duration::from_secs(2), left) {
            let now = self.fresh_screen()?;
            if self.request_is(&WaitKind::Permission, request_id)
                && self.screen_request(&now).is_some_and(|id| id == rid)
            {
                self.keys(n.to_string().as_bytes())?;
            }
        }
        let mut res = self.wait_leave_request(request_id)?;
        // The dialog's No interrupts the turn: the message goes as the next prompt.
        if let Some(msg) = message {
            if !self.wait_until(Duration::from_secs(10), |e| {
                e.state == State::Idle && !e.turns.active()
            }) {
                return err("timeout", "拒绝后 10 秒内没有回到 idle，说明没有发出");
            }
            let sent = self.send_prompt(msg)?;
            res["message_uuid"] = sent["uuid"].clone();
        }
        Ok(res)
    }

    /// Answers an AskUserQuestion through its waiting hook (the answers go to the tool as its
    /// input, as if picked in the dialog).
    pub(super) fn answer_question(&mut self, request_id: &str, answers: &[Answer]) -> CmdResult {
        self.check_request(WaitKind::Question, request_id)?;
        let held = self.requests.iter().position(|r| r.id == request_id);
        let Some(idx) = held.filter(|&i| {
            self.requests[i]
                .reply
                .as_ref()
                .is_some_and(|r| !hook::peer_gone(r))
        }) else {
            // No hook waits for it (asked before ccrpc attached, or the hook went away): keys.
            let input = match held {
                Some(i) => self.requests[i].input.clone(),
                None => match self
                    .tailer
                    .as_ref()
                    .and_then(|t| t.pending_by_id(request_id))
                {
                    Some(p) => p.input.clone(),
                    None => {
                        return err(
                            "unknown_request",
                            format!("当前挂起的请求不是 {request_id}"),
                        );
                    }
                },
            };
            return self.answer_question_on_screen(request_id, &input, answers);
        };
        let input = self.requests[idx].input.clone();
        let questions = input
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let picked = question_answers(&questions, answers).map_err(|m| ("bad_request", m))?;
        self.writable()?;
        let mut updated = input;
        updated["answers"] = Value::Object(picked);
        let stream = self.requests[idx].reply.take().expect("held");
        hook::send_reply(stream, Some(hook::allow_output(None, Some(updated))))
            .map_err(|e| ("io", format!("回复 hook 失败：{e}")))?;
        self.answered(request_id);
        self.wait_leave_request(request_id)
    }

    /// Answers an AskUserQuestion with keys on its dialog (no hook waits for it): each question's
    /// option number (a multi-select one toggles to the wanted set, then Submit), or `Type
    /// something.` then the text; several questions end on the Review tab.
    pub(super) fn answer_question_on_screen(
        &mut self,
        request_id: &str,
        input: &Value,
        answers: &[Answer],
    ) -> CmdResult {
        let questions = input
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Validates every answer before the first key.
        question_answers(&questions, answers).map_err(|m| ("bad_request", m))?;
        let mut plan = Vec::new();
        for (i, (q, a)) in questions.iter().zip(answers).enumerate() {
            let multi = q
                .get("multiSelect")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if multi && matches!(a, Answer::Other(_)) {
                return err(
                    "unsupported",
                    format!("第 {} 题是多选，按屏幕作答时不支持 other", i + 1),
                );
            }
            let title = q
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            plan.push((title, a, multi));
        }
        self.writable()?;
        let mut tabbed = false;
        for (title, a, multi) in plan {
            if !self.request_is(&WaitKind::Question, request_id) {
                return err(
                    "unknown_request",
                    format!("作答中请求已经变了，不再是 {request_id}"),
                );
            }
            tabbed |= self.answer_one_on_screen(&title, a, multi)?;
        }
        if tabbed {
            let review = self.wait_screen(Duration::from_secs(3), |_, s| {
                match recognize::ask(&s.lines) {
                    Some(recognize::AskScreen::Review { submit }) => Some(submit),
                    _ => None,
                }
            });
            let Some((submit, _)) = review else {
                return Err(self.unrecognized("answer_question", None));
            };
            self.keys(submit.to_string().as_bytes())?;
        }
        self.answered(request_id);
        self.wait_leave_request(request_id)
    }

    /// One question of [`Self::answer_question_on_screen`]; returns whether the dialog is tabbed.
    pub(super) fn answer_one_on_screen(
        &mut self,
        title: &str,
        a: &Answer,
        multi: bool,
    ) -> std::result::Result<bool, CmdErr> {
        let found = self.wait_screen(Duration::from_secs(3), |_, s| {
            match recognize::ask(&s.lines) {
                Some(recognize::AskScreen::Question(q))
                    if recognize::same_text(&q.title, title) =>
                {
                    Some(q)
                }
                _ => None,
            }
        });
        let Some((q, s)) = found else {
            return Err(self.unrecognized("answer_question", None));
        };
        match a {
            Answer::Other(text) => {
                let Some(n) = q.other else {
                    return Err(self.unrecognized("answer_question", Some(&s)));
                };
                self.keys(n.to_string().as_bytes())?;
                // Type only once the free-text field is active (digits would pick options).
                let editing =
                    self.wait_screen(Duration::from_secs(2), |_, s| {
                        match recognize::ask(&s.lines) {
                            Some(recognize::AskScreen::Question(q))
                                if recognize::same_text(&q.title, title) && q.editing =>
                            {
                                Some(())
                            }
                            _ => None,
                        }
                    });
                if editing.is_none() {
                    return Err(self.unrecognized("answer_question", None));
                }
                let clean = pty::sanitize(text).replace(['\n', '\t'], " ");
                self.keys(clean.as_bytes())?;
                self.keys(key::ENTER)?;
            }
            Answer::Options(chosen) if !multi => {
                let Some(o) = q.options.iter().find(|o| o.label == chosen[0]) else {
                    return Err(self.unrecognized("answer_question", Some(&s)));
                };
                self.keys(o.number.to_string().as_bytes())?;
            }
            Answer::Options(chosen) => {
                if q.options.len() != q.options.iter().filter(|o| o.checked.is_some()).count() {
                    return Err(self.unrecognized("answer_question", Some(&s)));
                }
                for o in &q.options {
                    if o.checked != Some(chosen.contains(&o.label)) {
                        self.keys(o.number.to_string().as_bytes())?;
                    }
                }
                if q.tabbed {
                    self.keys(key::RIGHT)?;
                } else {
                    // A single multi-select question: move the cursor onto the Submit row, Enter.
                    let (Some(sub), Some(cur)) = (q.submit_row, q.cursor_row) else {
                        return Err(self.unrecognized("answer_question", Some(&s)));
                    };
                    let rows_between = q
                        .options
                        .iter()
                        .filter(|o| o.row > cur && o.row < sub)
                        .count()
                        + usize::from(q.other.is_some());
                    for _ in 0..rows_between {
                        self.keys(key::DOWN)?;
                    }
                    let on_submit = self
                        .wait_screen(Duration::from_secs(2), |_, s| {
                            s.lines
                                .get(sub)
                                .filter(|l| recognize::on_submit_row(l))
                                .map(|_| ())
                        })
                        .is_some();
                    if !on_submit {
                        return Err(self.unrecognized("answer_question", None));
                    }
                    self.keys(key::ENTER)?;
                }
            }
        }
        Ok(q.tabbed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needles_for_dialog_matching() {
        assert_eq!(
            dialog_needle("Bash", &json!({"command": "touch  created.txt"})).as_deref(),
            Some("touchcreated.txt")
        );
        assert_eq!(
            dialog_needle("Edit", &json!({"file_path": "/a/b/c.rs"})).as_deref(),
            Some("c.rs")
        );
        assert_eq!(dialog_needle("mcp__x__y", &json!({"a": 1})), None);
    }

    #[test]
    fn question_answers_follow_the_dialog() {
        let qs = vec![
            json!({"question": "Which linters?", "multiSelect": true,
                "options": [{"label": "eslint"}, {"label": "ruff"}, {"label": "clippy"}]}),
            json!({"question": "Indent style?", "multiSelect": false,
                "options": [{"label": "Tabs"}, {"label": "Spaces"}]}),
        ];
        let a = question_answers(
            &qs,
            &[
                Answer::Options(vec!["eslint".into(), "clippy".into()]),
                Answer::Other("two\nspaces".into()),
            ],
        )
        .unwrap();
        assert_eq!(a["Which linters?"], "eslint, clippy");
        assert_eq!(a["Indent style?"], "two spaces");
        // Other on a multi-select is free text too.
        assert!(
            question_answers(
                &qs,
                &[
                    Answer::Other("all".into()),
                    Answer::Options(vec!["Tabs".into()])
                ]
            )
            .is_ok()
        );
        assert!(
            question_answers(
                &qs,
                &[
                    Answer::Options(vec!["nope".into()]),
                    Answer::Options(vec!["Tabs".into()])
                ]
            )
            .is_err()
        );
        assert!(
            question_answers(
                &qs,
                &[
                    Answer::Options(vec![]),
                    Answer::Options(vec!["Tabs".into()])
                ]
            )
            .is_err()
        );
        assert!(
            question_answers(
                &qs,
                &[
                    Answer::Options(vec!["ruff".into()]),
                    Answer::Options(vec!["Tabs".into(), "Spaces".into()])
                ]
            )
            .is_err(),
            "single choice"
        );
        assert!(
            question_answers(&qs[..1], &[]).is_err(),
            "one answer per question"
        );
    }
}
