//! `new --fork`: the Session started with `--resume <origin> --fork-session` is made into a Fork
//! after `ready` (CoDeck's ADR-0009): rewound to before a prompt (found in the Origin Session's
//! transcript: the new Session writes none until the rewind or `/rename`), renamed, and reported
//! once with a `fork` event. Commands that write wait until then.

use super::*;
use crate::rpc::RewindOption;

/// How long the forked claude may take to come up idle (time a screen waits for the caller's
/// answer does not count).
const FORK_START: Duration = Duration::from_secs(60);
/// How long the new Session may take to write its transcript after the rewind or `/rename`.
const FORK_WRITTEN: Duration = Duration::from_secs(10);
/// How long the new transcript's length must hold still to count as written.
const FORK_SETTLE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ForkPhase {
    /// Not started: waiting for the new Session to come up idle.
    Waiting,
    /// The rewind's confirm page (restoring code, with a warning) waits for `answer_rewind`.
    Confirming,
    /// Answering, renaming: nothing may start it again meanwhile.
    Finishing,
    Done,
}

#[derive(Debug, Clone)]
pub struct ForkJob {
    pub origin: String,
    pub origin_path: PathBuf,
    pub at: Option<String>,
    pub restore: RewindOption,
    pub title: Option<String>,
    /// The uuids of the Origin Session's entries: the new transcript starts with a copy of them.
    pub copied: HashSet<String>,
    pub(super) phase: ForkPhase,
    pub(super) since: Option<Instant>,
}

impl ForkJob {
    pub fn new(
        origin: String,
        origin_path: PathBuf,
        at: Option<String>,
        restore: RewindOption,
        title: Option<String>,
    ) -> ForkJob {
        ForkJob {
            copied: transcript::entry_uuids(&origin_path),
            origin,
            origin_path,
            at,
            restore,
            title,
            phase: ForkPhase::Waiting,
            since: None,
        }
    }
}

/// The option to press on the confirm page `event` (a `rewind` response) for `restore`, and
/// whether the caller must confirm it first: restoring code when the page warns (files are
/// overwritten whole). `both` falls back to `conversation` when the page does not offer it
/// (no code changed since the prompt).
pub(super) fn pick_option(event: &Value, restore: RewindOption) -> (RewindOption, bool) {
    let offered = |o: RewindOption| {
        event["options"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["option"] == o.name()))
    };
    if restore == RewindOption::Both && offered(RewindOption::Both) {
        (RewindOption::Both, !event["warning"].is_null())
    } else {
        (RewindOption::Conversation, false)
    }
}

/// What a failed start left on the screen: the last line with text above the cursor (claude's
/// error, such as `No conversation found with session ID: …`).
pub(super) fn exit_line(s: &Screen) -> Option<String> {
    let upto = (s.cursor.0 as usize).min(s.lines.len());
    s.lines[..upto]
        .iter()
        .map(|l| l.trim())
        .rfind(|l| !l.is_empty())
        .map(str::to_string)
}

impl Engine {
    fn set_fork_phase(&mut self, phase: ForkPhase) {
        if let Some(f) = self.fork.as_mut() {
            f.phase = phase;
        }
    }

    fn fork_phase(&self) -> Option<&ForkPhase> {
        self.fork.as_ref().map(|f| &f.phase)
    }

    /// The Fork is not made yet: commands that write wait for it (see [`Self::take_input_while_forking`]).
    pub(super) fn fork_holding(&self) -> bool {
        matches!(
            self.fork_phase(),
            Some(ForkPhase::Waiting | ForkPhase::Confirming)
        )
    }

    /// A Fork step is under way or still to come: no `abort` may cut into it.
    pub(super) fn fork_unfinished(&self) -> bool {
        self.fork_phase().is_some_and(|p| *p != ForkPhase::Done)
    }

    /// The copied entries of a Fork's transcript (empty when this is no Fork).
    pub(super) fn fork_copied(&self) -> HashSet<String> {
        self.fork
            .as_ref()
            .map(|f| f.copied.clone())
            .unwrap_or_default()
    }

    /// While the Fork is not made: reads, answers to what waits on the caller (a startup `choice`
    /// such as the trust folder dialog, keys, the Fork's own `answer_rewind`) and `exit` run now,
    /// ahead of lines held for after the Fork (those only run once it is made, so nothing is
    /// overtaken that could still run first); bad lines are answered at once. Returns false once
    /// stdin is closed.
    pub(super) fn take_input_while_forking(&mut self) -> bool {
        loop {
            let next = match &self.input {
                Some(rx) => rx.try_recv(),
                None => return false,
            };
            match next {
                Ok(line) => {
                    let now = match rpc::parse_line(&line) {
                        Ok(r) => {
                            serves_in_wait(&r.command)
                                || matches!(
                                    r.command,
                                    Command::AnswerChoice { .. }
                                        | Command::SendKeys { .. }
                                        | Command::Exit { .. }
                                        | Command::AnswerRewind { .. }
                                )
                        }
                        Err(_) => true,
                    };
                    if now {
                        self.handle_line(&line);
                    } else {
                        self.deferred.push_back(line);
                    }
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    /// One look while the Fork is not made: starts it once the new Session is up idle; ends it
    /// when claude exits meanwhile or its confirm page was answered in the TUI.
    pub(super) fn step_fork(&mut self) {
        match self.fork_phase() {
            Some(ForkPhase::Waiting) => self.start_fork(),
            Some(ForkPhase::Confirming) => self.watch_fork_confirm(),
            _ => {}
        }
    }

    fn start_fork(&mut self) {
        // Only what waits on the caller's answer (a request, a `choice`) stops the clock, and only
        // once the new Session is named: before that the caller cannot show it as the Fork's, and
        // the Fork fails with what is on the screen rather than wait for nobody.
        let answering = self.session_id.is_some()
            && (matches!(self.state, State::Waiting { .. }) || self.announced.is_some());
        let Some(job) = self.fork.as_mut() else {
            return;
        };
        if answering {
            job.since = Some(Instant::now());
        }
        let since = *job.since.get_or_insert_with(Instant::now);
        let (at, restore, origin_path) = (job.at.clone(), job.restore, job.origin_path.clone());
        if self.state == State::Exited {
            let line = self.screen().ok().and_then(|s| exit_line(&s));
            let msg = line.unwrap_or_else(|| "claude 已退出".into());
            self.fail_fork("exited", msg);
            return;
        }
        let up = self.session_id.is_some()
            && self.tailer.is_some()
            && self.state == State::Idle
            && self
                .screen()
                .is_ok_and(|s| recognize::input_text(&s).is_some());
        if !up {
            if since.elapsed() >= FORK_START {
                let line = self.screen().ok().and_then(|s| exit_line(&s));
                let msg = match line {
                    Some(l) => format!("60 秒内没有启动（{l}）"),
                    None => "60 秒内没有启动".into(),
                };
                self.fail_fork("timeout", msg);
            }
            return;
        }
        let Some(at) = at else {
            self.finish_fork(None);
            return;
        };
        crate::clog!("fork: rewinding to before {at}");
        self.set_fork_phase(ForkPhase::Finishing);
        let event = match self.rewind_in(&at, Some(&origin_path)) {
            Ok(e) => e,
            Err((code, msg)) => return self.fail_fork(code, msg),
        };
        let (option, confirm) = pick_option(&event, restore);
        let Some(id) = event["request_id"].as_str().map(str::to_string) else {
            return self.fail_fork("unrecognized_screen", "rewind 的确认页".into());
        };
        if confirm {
            // The caller sees the warning first: only the two options a Fork has.
            let mut shown = event.clone();
            if let Some(a) = shown["options"].as_array_mut() {
                a.retain(|o| o["option"] == "both" || o["option"] == "conversation");
            }
            if let Some(r) = self.rewind.as_mut() {
                r.event = shown.clone();
            }
            self.set_fork_phase(ForkPhase::Confirming);
            self.emit(shown);
            return;
        }
        match self.answer_rewind(&id, Some(option)) {
            Ok(v) => self.finish_fork(v["text"].as_str().map(str::to_string)),
            Err((code, msg)) => self.fail_fork(code, msg),
        }
    }

    /// `answer_rewind`; for the Fork's own rewind `cancel` means the conversation only, and the
    /// Fork is finished after it. Refused before any key (the page went away for a moment, say):
    /// the caller may answer again.
    pub(super) fn answer_rewind_cmd(
        &mut self,
        request_id: &str,
        option: Option<RewindOption>,
    ) -> CmdResult {
        let ours = self.fork_phase() == Some(&ForkPhase::Confirming)
            && self
                .rewind
                .as_ref()
                .is_some_and(|r| r.id.as_deref() == Some(request_id));
        if !ours {
            return self.answer_rewind(request_id, option);
        }
        let option = match option {
            Some(RewindOption::Code) => {
                return err("bad_request", "Fork 的回退只能恢复对话，或恢复代码与对话");
            }
            Some(o) => o,
            None => RewindOption::Conversation,
        };
        self.set_fork_phase(ForkPhase::Finishing);
        let r = self.answer_rewind(request_id, Some(option));
        match &r {
            Ok(v) => self.finish_fork(v["text"].as_str().map(str::to_string)),
            Err((code, _))
                if matches!(*code, "unknown_request" | "not_waiting")
                    && self.rewind.is_some()
                    && self.state != State::Exited =>
            {
                self.set_fork_phase(ForkPhase::Confirming);
            }
            Err((code, msg)) => self.fail_fork(code, msg.clone()),
        }
        r
    }

    /// While the Fork's confirm page waits: claude exited, or the page was left in the TUI. A
    /// conversation restored there wrote the new transcript (the Fork is what the TUI made of
    /// it); otherwise the rewind did not happen.
    fn watch_fork_confirm(&mut self) {
        if self.state == State::Exited {
            return self.fail_fork("exited", "claude 已退出".into());
        }
        if self.rewind.is_some() || self.state != State::Idle {
            return;
        }
        if !self
            .screen()
            .is_ok_and(|s| recognize::input_text(&s).is_some())
        {
            return;
        }
        let written = self.tailer.as_ref().is_some_and(|t| t.main_path().exists());
        if written {
            crate::clog!("fork: the rewind was answered in the TUI");
            self.finish_fork(None);
        } else {
            self.fail_fork("cancelled", "回退在终端里取消了".into());
        }
    }

    /// `/rename`, then wait for the new Session's transcript and report the Fork.
    fn finish_fork(&mut self, text: Option<String>) {
        let Some(job) = self.fork.clone() else {
            return;
        };
        self.set_fork_phase(ForkPhase::Finishing);
        let path = self.tailer.as_ref().map(|t| t.main_path().to_path_buf());
        let len_of = |p: &Option<PathBuf>| {
            p.as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
        };
        let before = len_of(&path);
        let (mut title, mut title_error) = (None, None);
        if let Some(t) = &job.title {
            // The prompt rewound to is back in the input box: cleared, the caller has its text.
            match self
                .clear_input()
                .and_then(|_| self.slash(&format!("rename {t}")))
            {
                Ok(_) => title = Some(t.clone()),
                Err((_, msg)) => title_error = Some(msg),
            }
        }
        if job.at.is_some() || title.is_some() {
            // Written in several appends (`/rename` after it): done once it grew past what was
            // there before the rename and its length holds still for a moment.
            let mut last: Option<(u64, Instant)> = None;
            let renamed = title.is_some();
            let written = path.as_ref().is_some_and(|p| {
                self.wait_until(FORK_WRITTEN, |_| {
                    let Ok(len) = std::fs::metadata(p).map(|m| m.len()) else {
                        return false;
                    };
                    if renamed && before.is_some_and(|b| len <= b) {
                        return false;
                    }
                    match last {
                        Some((l, since)) if l == len => since.elapsed() >= FORK_SETTLE,
                        _ => {
                            last = Some((len, Instant::now()));
                            false
                        }
                    }
                })
            });
            if self.state == State::Exited {
                return self.fail_fork("exited", "claude 已退出".into());
            }
            if !written {
                return self.fail_fork("timeout", "10 秒内新 Session 没有写出 transcript".into());
            }
        }
        self.set_fork_phase(ForkPhase::Done);
        crate::clog!("fork: done, session {:?}", self.session_id);
        self.emit(
            json!({"type": "fork", "session_id": self.session_id, "origin": job.origin,
            "text": text, "title": title, "title_error": title_error}),
        );
    }

    /// Reports the failure, answers the commands held for after the Fork, ends claude and its
    /// Pane (see [`Self::end_fork_pane`]) and ends ccrpc.
    fn fail_fork(&mut self, code: &str, message: String) {
        let origin = self.fork.as_ref().map(|f| f.origin.clone());
        self.set_fork_phase(ForkPhase::Done);
        crate::clog!("fork failed: {code} {message}");
        self.emit(json!({"type": "fork", "origin": origin,
            "error": {"code": code, "message": message}}));
        while let Some(line) = self.deferred.pop_front() {
            if let Ok(r) = rpc::parse_line(&line) {
                let name = rpc::command_name(&r.command);
                self.emit(rpc::response_err(
                    r.id.as_deref(),
                    name,
                    "exited",
                    "Fork 失败，ccrpc 已结束",
                ));
            }
        }
        self.end_fork_pane();
        self.fatal = Some(format!("Fork 失败：{message}"));
    }

    /// Ends the claude of a Fork not made (nothing of it is kept: before the rewind is answered
    /// the new Session has written no file) and its Pane: out of the Rewind picker, `/exit`, else
    /// ctrl+c twice (the TUI's own way out, from a startup screen too); once claude is gone,
    /// `exit` in the Pane's shell.
    pub(super) fn end_fork_pane(&mut self) {
        if self.state != State::Exited {
            self.leave_rewind();
            if let Err((_, e)) = self.exit(false, false) {
                crate::clog!("fork: /exit failed ({e}), ctrl+c");
                for _ in 0..2 {
                    let _ = self.keys(key::CTRL_C);
                    self.wait_until(Duration::from_millis(400), |e| e.state == State::Exited);
                }
                self.wait_until(Duration::from_secs(3), |e| e.state == State::Exited);
            }
        }
        if self.state == State::Exited {
            let _ = self.keys(&[b"exit".as_slice(), key::ENTER].concat());
        } else {
            crate::clog!("fork: claude did not exit, the Pane stays");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(options: &[&str], warning: Option<&str>) -> Value {
        json!({
            "request_id": "rewind:1",
            "options": options.iter().map(|o| json!({"option": o})).collect::<Vec<_>>(),
            "warning": warning,
        })
    }

    #[test]
    fn the_option_a_fork_presses() {
        use RewindOption::*;
        // The conversation only: never asks.
        assert_eq!(
            pick_option(
                &page(&["both", "conversation", "code"], Some("⚠")),
                Conversation
            ),
            (Conversation, false)
        );
        // Code too: asks only when the page warns.
        assert_eq!(
            pick_option(&page(&["both", "conversation", "code"], None), Both),
            (Both, false)
        );
        assert_eq!(
            pick_option(&page(&["both", "conversation", "code"], Some("⚠")), Both),
            (Both, true)
        );
        // No code changed since the prompt: the conversation is all there is.
        assert_eq!(
            pick_option(&page(&["conversation"], None), Both),
            (Conversation, false)
        );
    }

    #[test]
    fn the_error_left_on_the_screen() {
        let s = Screen {
            lines: vec![
                "$ command claude --resume x --fork-session".into(),
                "No conversation found with session ID: x".into(),
                "".into(),
                "$ ".into(),
                "".into(),
            ],
            cursor: (3, 2),
            ..Screen::default()
        };
        assert_eq!(
            exit_line(&s).as_deref(),
            Some("No conversation found with session ID: x")
        );
        let empty = Screen {
            lines: vec!["".into(), "$ ".into()],
            cursor: (1, 2),
            ..Screen::default()
        };
        assert_eq!(exit_line(&empty), None);
    }
}
