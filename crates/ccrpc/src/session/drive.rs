//! Writing to the Pane: keys and pastes checked against the screen, the takeover rule, and the
//! commands that type into the TUI.

use super::*;

impl Engine {
    pub fn screen(&self) -> Result<Screen> {
        Ok(self.conn.screen())
    }

    /// The screen from a Snapshot taken now (before a key whose meaning depends on the screen).
    pub(super) fn fresh_screen(&self) -> std::result::Result<Screen, CmdErr> {
        self.conn
            .resync(Duration::from_secs(3))
            .map_err(|e| ("io", format!("{e:#}")))
    }

    /// PTY size from the Pane shell's tty.
    pub(super) fn pty_size(&self) -> Option<(u16, u16)> {
        locate::tty_size(self.tty.as_deref()?).ok()
    }

    /// A size owner other than ccrpc means someone took the Pane over (a Holder that does not
    /// report owners: the PTY size changed). Afterwards the VT follows the PTY size.
    pub(super) fn check_takeover(&mut self) {
        // Co-display: the VT just follows the PTY size whoever set it.
        if self.co_display {
            if self.last_size_check.elapsed() >= Duration::from_secs(1) {
                self.last_size_check = Instant::now();
                if let Some(size) = self.pty_size()
                    && size != self.conn.vt_size()
                {
                    let _ = self.conn.follow_size(size.0, size.1);
                    self.size = size;
                }
            }
            return;
        }
        if !self.taken_over {
            // Sticky: someone who took the size and left between two looks still took over.
            if let Some(client) = self.conn.taken_by() {
                return self.take_over(Some(client));
            }
            if self.conn.owner() != Owner::Unknown {
                return;
            }
        }
        if self.last_size_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_size_check = Instant::now();
        let Some(size) = self.pty_size() else {
            return;
        };
        if !self.taken_over {
            if size != self.size {
                self.take_over(None);
            }
        } else if size != self.conn.vt_size() {
            let _ = self.conn.follow_size(size.0, size.1);
        }
    }

    pub(super) fn take_over(&mut self, client: Option<String>) {
        self.taken_over = true;
        self.follow_ups.clear();
        self.release_hooks();
        self.emit(json!({"type": "taken_over", "client": client}));
        if let Some((cols, rows)) = self.pty_size() {
            let _ = self.conn.follow_size(cols, rows);
        }
    }

    /// Ticks until `cond` holds or `timeout` passes.
    pub(super) fn wait_until(
        &mut self,
        timeout: Duration,
        mut cond: impl FnMut(&Engine) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let seen = self.wake.seen();
            self.tick();
            if cond(self) {
                return true;
            }
            if self.exited || Instant::now() >= deadline {
                return false;
            }
            self.wake.wait(seen, TICK);
        }
    }

    /// Polls the screen until `f` returns `Some`.
    pub(super) fn wait_screen<T>(
        &mut self,
        timeout: Duration,
        mut f: impl FnMut(&Engine, &Screen) -> Option<T>,
    ) -> Option<(T, Screen)> {
        let deadline = Instant::now() + timeout;
        loop {
            let seen = self.conn.generation();
            self.tick();
            if let Ok(s) = self.screen()
                && let Some(v) = f(self, &s)
            {
                return Some((v, s));
            }
            if self.exited || Instant::now() >= deadline {
                return None;
            }
            self.conn.wait_change(seen, TICK);
        }
    }

    /// Every write goes through here or [`Self::paste`]: none once someone took the Pane over,
    /// even in the middle of a command.
    pub(super) fn writable(&self) -> std::result::Result<(), CmdErr> {
        // The Holder's report counts at once, before a tick turns it into `taken_over`.
        if self.taken_over || (!self.co_display && self.conn.taken_by().is_some()) {
            return err("taken_over", "Session 已被人接手，只读");
        }
        Ok(())
    }

    pub(super) fn paste(&self, text: &str) -> std::result::Result<(), CmdErr> {
        self.writable()?;
        crate::clog!(
            "paste {} chars: {:?}",
            text.chars().count(),
            text.chars().take(60).collect::<String>()
        );
        self.conn.paste(text).map_err(|e| ("io", format!("{e:#}")))
    }

    pub(super) fn keys(&self, bytes: &[u8]) -> std::result::Result<(), CmdErr> {
        self.writable()?;
        crate::clog!("keys {:?}", String::from_utf8_lossy(bytes));
        let seen = self.conn.generation();
        self.conn
            .input(bytes)
            .map_err(|e| ("io", format!("{e:#}")))?;
        std::thread::sleep(KEY_MIN_GAP);
        self.conn.wait_change(seen, KEY_SETTLE - KEY_MIN_GAP);
        Ok(())
    }

    pub(super) fn unrecognized(&self, context: &str, s: Option<&Screen>) -> CmdErr {
        let lines = match s {
            Some(s) => s.lines.clone(),
            None => self.screen().map(|s| s.lines).unwrap_or_default(),
        };
        self.emit(json!({"type": "unrecognized", "context": context, "lines": lines}));
        (
            "unrecognized_screen",
            format!("{context}：屏幕与已知布局不符"),
        )
    }

    /// The prompt input box is on screen (no dialog has it): returns its text.
    pub(super) fn input_box_now(&self) -> std::result::Result<String, CmdErr> {
        let s = self.screen().map_err(|e| ("io", format!("{e:#}")))?;
        recognize::input_text(&s).ok_or_else(|| self.unrecognized("输入框", Some(&s)))
    }

    /// Empties the prompt input box (`ctrl+u` + backspace per line), checking the screen.
    pub(super) fn clear_input(&mut self) -> std::result::Result<(), CmdErr> {
        // Just after startup the Process File says idle before the TUI is drawn.
        self.wait_screen(Duration::from_secs(2), |_, s| {
            recognize::input_text(s).map(|_| ())
        });
        for _ in 0..3 {
            let text = self.input_box_now()?;
            if text.is_empty() {
                return Ok(());
            }
            let n = text.lines().count().max(1);
            let mut b = Vec::new();
            for i in 0..n {
                b.extend_from_slice(key::CTRL_U);
                if i + 1 < n {
                    b.extend_from_slice(key::BACKSPACE);
                }
            }
            self.keys(&b)?;
            self.wait_screen(Duration::from_millis(500), |_, s| {
                recognize::input_text(s).filter(|t| t.is_empty())
            });
        }
        if self.input_box_now()?.is_empty() {
            return Ok(());
        }
        if let Ok(s) = self.screen() {
            crate::clog!("input box not cleared");
            crate::log::screen(s.lines.iter().map(String::as_str));
        }
        err("input_not_empty", "输入框清不空")
    }

    /// Pastes `clean` and presses Enter, checking before each step that the input box (not a
    /// dialog) has the screen, so Enter cannot answer a dialog.
    pub(super) fn paste_and_submit(&mut self, clean: &str) -> std::result::Result<(), CmdErr> {
        let before = self.input_box_now()?;
        self.paste(clean)?;
        // Long pastes show as a `[Pasted text …]` placeholder: wait for the box to change.
        self.wait_screen(Duration::from_secs(2), |_, s| {
            recognize::input_text(s).filter(|t| !t.is_empty() && *t != before)
        });
        self.input_box_now()?;
        self.keys(key::ENTER)
    }

    pub(super) fn require(&self, ok: bool, code: &'static str) -> std::result::Result<(), CmdErr> {
        if self.state == State::Exited {
            return err("exited", "Session 已结束");
        }
        if ok {
            Ok(())
        } else {
            err(code, format!("当前状态为 {}", self.state.name()))
        }
    }

    pub(super) fn clean_text(text: &str) -> std::result::Result<String, CmdErr> {
        let clean = pty::sanitize(text);
        if clean.trim().is_empty() {
            return err("bad_request", "去掉控制字符后正文为空");
        }
        Ok(clean)
    }

    pub(super) fn send_prompt(&mut self, text: &str) -> CmdResult {
        self.send_prompt_with(text, &[])
    }

    /// Image paths made absolute (Claude Code resolves a relative one against its own cwd); each
    /// must be an existing file with no control characters.
    pub(super) fn image_paths(images: &[String]) -> std::result::Result<Vec<String>, CmdErr> {
        let mut paths = Vec::new();
        for p in images {
            if p.chars().any(char::is_control) {
                return Err(("bad_request", format!("图片路径里有控制字符：{p:?}")));
            }
            match std::path::absolute(p) {
                Ok(abs) if abs.is_file() => paths.push(abs.display().to_string()),
                _ => return Err(("bad_request", format!("图片文件不存在：{p}"))),
            }
        }
        Ok(paths)
    }

    pub(super) fn send_prompt_with(&mut self, text: &str, images: &[String]) -> CmdResult {
        // Idle and no turn open. A turn that went idle without its Stop (interrupted) closes within
        // a few seconds: wait for that.
        if self.state == State::Idle && self.turn_active {
            self.wait_until(Duration::from_secs(6), |e| {
                !e.turn_active || e.state != State::Idle
            });
        }
        self.require(self.state == State::Idle && !self.turn_active, "not_idle")?;
        let clean = Self::clean_text(text)?;
        let paths = Self::image_paths(images)?;
        let images = &paths;
        self.clear_input()?;
        self.paste_images(images)?;
        let before = self.marks.seq;
        let hook_before = self.hook_seq;
        self.paste_and_submit(&clean)?;
        let found = |e: &Engine| -> Option<String> {
            if images.is_empty() {
                e.marks.prompt_after(before, &clean).map(str::to_string)
            } else {
                e.marks
                    .prompt_with_images_after(before, &clean)
                    .map(str::to_string)
            }
        };
        // Claude Code may have started a turn just before Enter: then the prompt waits in its queue.
        let queued = |e: &Engine| {
            e.marks
                .prompt_enqueued_after(before, &clean, !images.is_empty())
        };
        let ok = self.wait_until(Duration::from_secs(30), |e| {
            found(e).is_some() || queued(e) || e.marks.blank_enqueued_after(before)
        });
        if !ok {
            return err("timeout", "30 秒内没在 transcript 里看到这条提示词");
        }
        if found(self).is_none() && !queued(self) {
            // A blank enqueue may be another session's message: it is this prompt only when no
            // user entry follows at once and the text has left the input box.
            self.wait_until(Duration::from_secs(2), |e| found(e).is_some() || queued(e));
            let needle: String = clean
                .trim()
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(20)
                .collect();
            let still_typed = self
                .screen()
                .ok()
                .and_then(|s| recognize::input_text(&s))
                .is_some_and(|t| t.contains(&needle));
            if found(self).is_none() && !queued(self) && still_typed {
                return err("timeout", "回车后提示词还在输入框里");
            }
        }
        if found(self).is_none() {
            crate::clog!("prompt went into Claude Code's queue");
            return Ok(json!({"uuid": null, "prompt_id": null, "delivered_as": "queued"}));
        }
        // The plugin saw it submitted, normally before it reached the transcript.
        let hook_prompt = |e: &Engine| {
            e.hook_prompts
                .iter()
                .rev()
                .find(|(seq, _, t)| *seq > hook_before && t.trim().ends_with(clean.trim()))
                .map(|(_, p, _)| p.clone())
        };
        if hook_prompt(self).is_none() {
            self.wait_until(Duration::from_secs(1), |e| hook_prompt(e).is_some());
        }
        let prompt_id = hook_prompt(self);
        Ok(json!({"uuid": found(self), "prompt_id": prompt_id, "delivered_as": "prompt"}))
    }

    /// Images are pasted by path, as a terminal drop would; each shows as `[Image #n]`.
    pub(super) fn paste_images(&mut self, paths: &[String]) -> std::result::Result<(), CmdErr> {
        for p in paths {
            let had = recognize::image_placeholders(&self.input_box_now()?);
            self.paste(p)?;
            // One more `[Image #n]` placeholder, and the path itself is gone from the box.
            let shown = self.wait_screen(Duration::from_secs(3), |_, s| {
                recognize::input_text(s).filter(|t| {
                    !t.contains(p.as_str()) && recognize::image_placeholders(t) == had + 1
                })
            });
            if shown.is_none() {
                return Err(("bad_request", format!("Claude Code 没有把 {p} 当成图片")));
            }
        }
        Ok(())
    }

    pub(super) fn steer(&mut self, text: &str, images: &[String]) -> CmdResult {
        self.require(matches!(self.state, State::Busy), "not_busy")?;
        let clean = Self::clean_text(text)?;
        let paths = Self::image_paths(images)?;
        self.clear_input()?;
        self.paste_images(&paths)?;
        let with_images = !paths.is_empty();
        let before = self.marks.seq;
        self.paste_and_submit(&clean)?;
        let mut how = None;
        self.wait_until(Duration::from_secs(10), |e| {
            // With images, the recorded text starts with their `[Image #n]` placeholders.
            let prompt = if with_images {
                e.marks.prompt_with_images_after(before, &clean)
            } else {
                e.marks.prompt_after(before, &clean)
            };
            if e.marks.prompt_enqueued_after(before, &clean, with_images) {
                how = Some("queued");
            } else if prompt.is_some() {
                how = Some("prompt");
            }
            how.is_some()
        });
        match how {
            Some(h) => Ok(json!({"delivered_as": h})),
            None => err("timeout", "10 秒内没在 transcript 里看到这条插话"),
        }
    }

    pub(super) fn abort(&mut self) -> CmdResult {
        self.require(
            matches!(self.state, State::Busy | State::Waiting { .. }),
            "not_busy",
        )?;
        self.keys(key::ESC)?;
        if self.wait_until(Duration::from_secs(10), |e| {
            !matches!(e.state, State::Busy | State::Waiting { .. })
        }) {
            Ok(json!({"state": self.state.name()}))
        } else {
            err("timeout", "10 秒内没有停下")
        }
    }

    /// Sends what waits in Claude Code's queue now, as `ctrl+enter` in the TUI: the running turn is
    /// interrupted and the queue runs as the next prompt.
    pub(super) fn send_now(&mut self) -> CmdResult {
        self.require(self.state == State::Busy, "not_busy")?;
        if self.queue.is_empty() {
            return err("bad_request", "队列里没有要发的内容");
        }
        self.writable()?;
        // The running turn ends here (an interrupted turn gets no Stop); the queue runs as a turn
        // of its own with no UserPromptSubmit, ended by its Stop (as a steer delivered after Stop).
        if self.turn_active {
            self.turn.aborted = true;
            self.emit_turn_end(None, Value::Null);
        }
        self.pending_steers.clear();
        self.turn = TurnStats::start(None, "prompt");
        self.turn.provisional = true;
        self.turn_active = true;
        self.last_text = None;
        self.keys(key::CTRL_ENTER)?;
        if self.wait_until(Duration::from_secs(5), |e| e.queue.is_empty()) {
            Ok(json!({}))
        } else {
            err("timeout", "5 秒内队列里的内容没有发出")
        }
    }

    pub(super) fn slash(&mut self, command: &str) -> CmdResult {
        self.require(self.state == State::Idle, "not_idle")?;
        let clean = Self::clean_text(command)?;
        if clean.contains(['\n', '\t']) {
            return err("bad_request", "斜杠命令必须是单行");
        }
        self.clear_input()?;
        let typed = format!("/{}", clean.trim());
        self.keys(typed.as_bytes())?;
        self.wait_screen(Duration::from_secs(1), |_, s| {
            recognize::input_text(s).filter(|t| t.starts_with(&typed))
        });
        self.input_box_now()?;
        self.keys(key::ENTER)?;
        // Submitted once the command left the input box (or a picker replaced the box).
        self.wait_screen(Duration::from_secs(2), |_, s| {
            match recognize::input_text(s) {
                Some(t) if t.starts_with(&typed) => None,
                _ => Some(()),
            }
        });
        Ok(json!({}))
    }

    pub(super) fn send_keys(&mut self, keys: &[rpc::Key]) -> CmdResult {
        self.require(true, "")?;
        for k in keys {
            self.keys(&rpc::key_to_bytes(k))?;
        }
        Ok(json!({}))
    }

    pub(super) fn follow_up_texts(&self) -> Vec<&str> {
        self.follow_ups.iter().map(|(t, _)| t.as_str()).collect()
    }

    pub(super) fn cancel_follow_up(&mut self, index: Option<usize>) -> CmdResult {
        let removed = match index {
            Some(i) if i < self.follow_ups.len() => {
                self.follow_ups.remove(i);
                1
            }
            Some(i) => {
                return err(
                    "bad_request",
                    format!(
                        "follow_up 队列只有 {} 条，没有第 {i} 条",
                        self.follow_ups.len()
                    ),
                );
            }
            None => std::mem::take(&mut self.follow_ups).len(),
        };
        Ok(json!({"removed": removed, "follow_ups": self.follow_up_texts()}))
    }

    /// Takes the Session back after a takeover: without `force` only once the person's client
    /// is gone (no size owner online).
    pub(super) fn reclaim(&mut self, force: bool) -> CmdResult {
        if self.state == State::Exited {
            return err("exited", "Session 已结束");
        }
        if !self.taken_over {
            return err("not_taken_over", "Session 没有被接手");
        }
        match self.conn.owner() {
            Owner::Me | Owner::Nobody => {}
            Owner::Other(c) if !force => {
                return err(
                    "taken_over",
                    format!("Session 正被 {c} 使用，加 force 强制取回"),
                );
            }
            Owner::Unknown if !force => {
                return err(
                    "taken_over",
                    "Holder 不报告尺寸所有者，无法确认对方已离开，加 force 强制取回",
                );
            }
            _ => {}
        }
        self.conn.clear_taken_by();
        self.conn
            .claim_size(self.size.0, self.size.1)
            .map_err(|e| ("io", format!("{e:#}")))?;
        self.taken_over = false;
        self.last_size_check = Instant::now();
        self.emit(json!({"type": "reclaimed"}));
        Ok(json!({"size": [self.size.0, self.size.1]}))
    }

    pub(super) fn mode_now(&self) -> Option<rpc::PermissionMode> {
        self.screen()
            .ok()
            .and_then(|s| recognize::permission_mode(&s.lines))
    }

    /// Cycles the permission mode with shift+tab until the footer shows `mode`.
    pub(super) fn set_mode(&mut self, mode: rpc::PermissionMode) -> CmdResult {
        // Idle only: on the way the cycle may pass through auto (or bypass) mode.
        self.require(self.state == State::Idle, "not_idle")?;
        self.input_box_now()?;
        let Some(start) = self.mode_now() else {
            return Err(self.unrecognized("set_mode", None));
        };
        let mut cur = start;
        for _ in 0..=rpc::PermissionMode::CYCLE.len() {
            if cur == mode {
                return Ok(json!({"mode": mode.name()}));
            }
            self.keys(key::SHIFT_TAB)?;
            let was = cur;
            let next = self.wait_screen(Duration::from_secs(2), |_, s| {
                recognize::permission_mode(&s.lines).filter(|m| *m != was)
            });
            let Some((m, _)) = next else {
                let now = self.mode_now().map_or("未知", |m| m.name());
                self.unrecognized("set_mode", None);
                return err(
                    "unrecognized_screen",
                    format!("set_mode：模式行没有变化，当前为 {now}"),
                );
            };
            cur = m;
            // Never stop on bypass permissions: it is passed through, not a target.
            if cur == start {
                break;
            }
        }
        if cur == rpc::PermissionMode::Bypass {
            self.unrecognized("set_mode", None);
            return err(
                "unrecognized_screen",
                "set_mode：停在了 bypass permissions，当前为 bypass",
            );
        }
        if cur == mode {
            return Ok(json!({"mode": mode.name()}));
        }
        err("not_offered", format!("权限模式里没有 {}", mode.name()))
    }

    /// `/exit`, then wait for claude to be gone; `close_pane` also ends the Pane's shell.
    pub(super) fn exit(&mut self, close_pane: bool, stop_tasks: bool) -> CmdResult {
        if self.state != State::Exited {
            self.require(self.state == State::Idle, "not_idle")?;
            let Some(pid) = self.claude_pid else {
                return err("not_idle", "还没找到 claude 进程");
            };
            self.slash("exit")?;
            // Background tasks still running make Claude Code ask first: answered only when the
            // question is the known one (`Some(cursor on "Exit and stop tasks")`).
            let asked = self.wait_screen(Duration::from_secs(10), |_, s| {
                if !::transcript::live::pid_exists(pid) {
                    return Some(None);
                }
                recognize::exit_with_tasks(&s.lines).map(Some)
            });
            match asked {
                Some((Some(_), _)) if !stop_tasks => {
                    self.keys(key::ESC)?;
                    return err(
                        "not_idle",
                        "还有后台任务在运行：加 stop_tasks 退出并停掉它们",
                    );
                }
                Some((Some(true), _)) => self.keys(key::ENTER)?,
                Some((Some(false), s)) => return Err(self.unrecognized("exit", Some(&s))),
                // Gone, or neither gone nor asking what is known: no key; wait for it to exit.
                Some((None, _)) | None => {}
            }
            if !self.wait_until(Duration::from_secs(15), |_| {
                !::transcript::live::pid_exists(pid)
            }) {
                return err("timeout", "15 秒内 claude 没有退出");
            }
            self.set_exited();
        }
        if close_pane {
            // The Pane is a login shell that ran claude; ending it ends the Pane.
            self.keys(&[b"exit".as_slice(), key::ENTER].concat())?;
        }
        Ok(json!({}))
    }

    /// Sends the next follow-up when the Session is idle with no turn open. Only called from the
    /// top-level loop.
    pub(super) fn drain_follow_ups(&mut self) {
        if self.state != State::Idle
            || self.turn_active
            || self.idle_at.is_none_or(|t| t.elapsed() < FOLLOW_UP_SETTLE)
        {
            return;
        }
        let Some((text, images)) = self.follow_ups.pop_front() else {
            return;
        };
        if let Err((code, msg)) = self.send_prompt_with(&text, &images) {
            self.emit(json!({"type": "follow_up_failed", "text": text, "error": {"code": code, "message": msg}}));
        }
    }
}
