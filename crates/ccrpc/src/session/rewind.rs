//! `rewind` / `answer_rewind`: the TUI's Rewind picker (`Esc Esc` on an empty input box) driven
//! to the confirm page of a Main Line prompt, which then waits for the caller (docs/probes.md).
//! The picker lists typed prompts and slash and `!` commands (as CoDeck shows them), each by its
//! first line only, so a row is only a candidate: walking up from `(current)`, every row whose
//! text fits is opened, and its confirm page (the prompt quoted, when it was sent) must fit the
//! target before anything is offered. Pages that fit another prompt are left with Esc. Prompts
//! alike in text and time are told apart by order (the picker lists every prompt, newest last),
//! unless one of them was typed while busy (it may not be listed): then `ambiguous`.

use super::*;
use crate::recognize::{RewindConfirm, rewind_confirm, rewind_picker};
use crate::rpc::RewindOption;

/// A rewind under way. While ccrpc drives the screens `id` is `None`; once the confirm page is
/// up it is announced as `rewind:N` and waits for `answer_rewind`.
#[derive(Debug, Clone)]
pub(super) struct RewindReq {
    pub(super) id: Option<String>,
    uuid: String,
    /// The prompt's full text (it returns to the input box when the conversation is restored).
    text: String,
    /// The confirm page as announced: the screen shows this rewind while it shows this page.
    page: Option<RewindConfirm>,
    /// What `rewind` answered (and `get_state` shows as the request).
    pub(super) event: Value,
    /// Since when the Session has not been waiting on this page.
    gone_since: Option<Instant>,
}

/// A Main Line prompt as the rewind needs it.
struct Prompt {
    uuid: String,
    text: String,
    /// Typed while busy and delivered within a turn: not a prompt of its own, but it may still
    /// show in the picker.
    queued: bool,
    /// When it was sent (unix ms).
    at_ms: Option<f64>,
    /// A slash or `!` command (whether one was typed while busy is not known).
    command: bool,
}

/// How long the announced confirm page may be away (the Process File flickers; someone answered
/// or pressed Esc in the TUI) before the rewind counts as over.
const CONFIRM_GONE: Duration = Duration::from_millis(500);

/// Slack between the age a confirm page shows (`5s ago`, as drawn) and the transcript's time.
const AGE_SLACK_SECS: f64 = 3.0;

fn ambiguous() -> CmdResult {
    err(
        "ambiguous",
        "终端的回退列表里有分不清的同样提示词，请在终端里回退",
    )
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or_default()
}

/// Indices of the prompts a confirm page's quote may be about.
fn quoting(prompts: &[Prompt], page: &RewindConfirm) -> Vec<usize> {
    (0..prompts.len())
        .filter(|&i| recognize::rewind_quote_matches(&page.quote, &prompts[i].text))
        .collect()
}

/// The page's age (`5s ago`) fits when the prompt was sent; true when either is unknown.
fn age_fits(page: &RewindConfirm, prompt: &Prompt, now_ms: f64) -> bool {
    match (
        page.ago.as_deref().and_then(recognize::rewind_age),
        prompt.at_ms,
    ) {
        (Some((lo, hi)), Some(at)) => {
            let secs = (now_ms - at) / 1000.0;
            secs >= lo as f64 - AGE_SLACK_SECS && secs < hi as f64 + AGE_SLACK_SECS
        }
        _ => true,
    }
}

/// The prompts of the Main Line as CoDeck shows them (the transcript library's assembly, no
/// Branch chosen): what a person typed, and slash and `!` commands written as the picker lists
/// them (`/greet world`, `! echo bang`). A typed prompt's text is the transcript's own (the
/// assembled one may be cut short).
fn main_line_prompts(path: &Path) -> Vec<Prompt> {
    use ::transcript::assemble::{LoadedFile, SessionSidecar};
    use ::transcript::model::{
        FileRole, NodeBody, PromptOrigin, TranscriptRequest, TranscriptScope,
    };
    let Ok(chunk) = ::transcript::parse::read_entries(path, 0) else {
        return Vec::new();
    };
    let sid = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = LoadedFile {
        path: path.to_path_buf(),
        role: FileRole::Main,
        agent_id: None,
        records: chunk.records,
    };
    let session = ::transcript::assemble::assemble(
        &sid,
        &path.with_extension(""),
        vec![file],
        SessionSidecar::default(),
    );
    let req = TranscriptRequest {
        session_id: sid,
        scope: TranscriptScope::Main,
        branch_choices: Vec::new(),
        include_hidden: false,
    };
    let Ok(view) = ::transcript::assemble::transcript(&session, &req, None) else {
        return Vec::new();
    };
    // ccrpc's own reading of the prompts: whole texts, and which were typed while busy.
    let (blocks, _) = transcript::main_blocks(path, 0);
    let mut typed: HashMap<String, (Option<String>, bool)> = HashMap::new();
    for b in blocks.into_iter().filter(|b| b.kind == BlockKind::User) {
        // A prompt's first text block is what was typed (later ones: an opened file…).
        typed.entry(b.uuid).or_insert((b.text, b.queued));
    }
    view.nodes
        .into_iter()
        .filter_map(|n| {
            let NodeBody::UserPrompt { text, origin, .. } = n.body else {
                return None;
            };
            let command = matches!(origin, PromptOrigin::Command { .. });
            let text = match origin {
                PromptOrigin::Human => typed.get(&n.id).and_then(|t| t.0.clone()).unwrap_or(text),
                PromptOrigin::Command { name, args } if name == "!" => {
                    format!("! {}", args.trim())
                }
                PromptOrigin::Command { name, args } => {
                    format!("{name} {}", args.trim()).trim_end().to_string()
                }
                _ => return None,
            };
            Some(Prompt {
                queued: typed.get(&n.id).is_some_and(|t| t.1),
                at_ms: n.timestamp_ms,
                uuid: n.id,
                text,
                command,
            })
        })
        .collect()
}

impl Engine {
    /// Main Line prompts and commands in order, read on a thread (a long transcript takes a while; reads are
    /// served meanwhile).
    fn main_prompts(&mut self) -> std::result::Result<Vec<Prompt>, CmdErr> {
        let Some(path) = self.tailer.as_ref().map(|t| t.main_path().to_path_buf()) else {
            return err("not_found", "Session 还没有 transcript");
        };
        let (tx, rx) = mpsc::channel();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let _ = tx.send(main_line_prompts(&path));
            wake.bump();
        });
        let mut got = None;
        self.wait_until(Duration::from_secs(60), |_| {
            if got.is_none() {
                got = rx.try_recv().ok();
            }
            got.is_some()
        });
        got.ok_or(("timeout", "60 秒内没读完 transcript".to_string()))
    }

    pub(super) fn rewind(&mut self, uuid: &str) -> CmdResult {
        self.require(self.state == State::Idle, "not_idle")?;
        let mut prompts = self.main_prompts()?;
        // Restoring the conversation shows in the transcript only with the next prompt: until then
        // the TUI's conversation ends before the prompt rewound to.
        if let Some(cut) = self
            .rewound
            .as_ref()
            .and_then(|u| prompts.iter().position(|p| &p.uuid == u))
        {
            prompts.truncate(cut);
        }
        let Some(target) = prompts.iter().position(|p| p.uuid == uuid && !p.queued) else {
            return err("not_found", "这条不是 Main Line 上的提示词");
        };
        // The Session may have moved on while the transcript was read: Esc would interrupt it.
        self.require(self.state == State::Idle, "not_idle")?;
        self.rewind = Some(RewindReq {
            id: None,
            uuid: uuid.to_string(),
            text: prompts[target].text.clone(),
            page: None,
            event: Value::Null,
            gone_since: None,
        });
        let r = self.drive_rewind(&prompts, target);
        if r.is_err() {
            // Still ours while backing out: nothing is announced for the picker meanwhile.
            self.leave_rewind();
            self.rewind = None;
        }
        r
    }

    /// Waits for the picker with its cursor on row `pos` (from the top).
    fn picker_at(&mut self, pos: usize, timeout: Duration) -> Option<recognize::RewindPicker> {
        self.wait_screen(timeout, |_, s| {
            rewind_picker(&s.lines).filter(|p| p.position() == Some(pos))
        })
        .map(|(p, _)| p)
    }

    fn drive_rewind(&mut self, prompts: &[Prompt], target: usize) -> CmdResult {
        let text = prompts[target].text.clone();
        self.clear_input()?;
        // A turn may have started meanwhile: Esc would interrupt it.
        self.require(self.state == State::Idle, "not_idle")?;
        self.keys(key::ESC)?;
        self.keys(key::ESC)?;
        let Some((picker, _)) =
            self.wait_screen(Duration::from_secs(3), |_, s| rewind_picker(&s.lines))
        else {
            return Err(self.unrecognized("rewind 的选择器", None));
        };
        if picker.cursor() != Some(recognize::REWIND_CURRENT) {
            return Err(self.unrecognized("rewind 的选择器", None));
        }
        let Some(mut pos) = picker.position() else {
            return Err(self.unrecognized("rewind 的选择器", None));
        };
        // Pages met so far per group of prompts quoted alike.
        let mut met: HashMap<Vec<usize>, usize> = HashMap::new();
        while pos > 0 {
            self.keys(key::UP)?;
            pos -= 1;
            let Some(picker) = self.picker_at(pos, Duration::from_secs(1)) else {
                return Err(self.unrecognized("rewind 的选择器（光标没有上移一行）", None));
            };
            let row = picker.cursor().unwrap_or_default().to_string();
            if !recognize::rewind_row_matches(&row, &text) {
                continue;
            }
            // A candidate: its confirm page tells which prompt it is.
            self.keys(key::ENTER)?;
            let Some((page, _)) =
                self.wait_screen(Duration::from_secs(3), |_, s| rewind_confirm(&s.lines))
            else {
                return Err(self.unrecognized("rewind 的确认页", None));
            };
            // Prompts quoted alike are met newest first: the page stands for the next of them.
            let group = quoting(prompts, &page);
            let this = if !group.contains(&target) {
                None
            } else if group.len() == 1 {
                Some(target)
            } else {
                // Typed while busy, one of them may not be listed: the order says nothing (for a
                // command it is not known whether it was).
                if group
                    .iter()
                    .any(|&i| prompts[i].queued || prompts[i].command)
                {
                    return ambiguous();
                }
                let k = met.entry(group.clone()).or_default();
                let Some(i) = group.len().checked_sub(1 + *k) else {
                    return ambiguous();
                };
                *k += 1;
                // Each page met must be sent when its prompt was: otherwise the picker lists
                // other prompts than the transcript (rewound in the TUI, say) and the order lies.
                if !age_fits(&page, &prompts[group[i]], now_ms()) {
                    return ambiguous();
                }
                Some(group[i])
            };
            crate::clog!(
                "rewind candidate {row:?} ({:?}) quotes {:?}, stands for {:?}",
                page.ago,
                group.iter().map(|&i| &prompts[i].uuid).collect::<Vec<_>>(),
                this.map(|i| &prompts[i].uuid)
            );
            if this == Some(target) {
                // When it was sent must agree too, or the page is not what the transcript says.
                if !age_fits(&page, &prompts[target], now_ms()) {
                    return ambiguous();
                }
                return self.open_confirm(page);
            }
            // Another prompt's page: back to the picker, its cursor where it was.
            self.keys(key::ESC)?;
            if self.picker_at(pos, Duration::from_secs(2)).is_none() {
                return Err(self.unrecognized("rewind 的选择器", None));
            }
        }
        err("not_in_picker", "终端的回退列表里没有这条提示词")
    }

    /// The target's confirm page is up: its `Restore …` options are what the caller may pick.
    fn open_confirm(&mut self, page: RewindConfirm) -> CmdResult {
        let options: Vec<Value> = RewindOption::ALL
            .into_iter()
            .filter(|o| page.options.iter().any(|(_, l, _)| l == o.label()))
            .map(|o| json!({"option": o.name(), "label": o.label()}))
            .collect();
        if options.is_empty() {
            return Err(self.unrecognized("rewind 的确认页", None));
        }
        let Some(r) = &mut self.rewind else {
            return err("unknown_request", "回退已经结束");
        };
        self.rewind_seq += 1;
        let id = format!("rewind:{}", self.rewind_seq);
        let event = json!({"type": "rewind", "request_id": id, "uuid": r.uuid, "text": r.text,
            "code": page.code, "warning": page.warning, "options": options});
        crate::clog!("rewind {id} to {}: {:?}", r.uuid, page.code);
        r.id = Some(id);
        r.page = Some(page);
        r.event = event.clone();
        if let Ok(s) = self.screen() {
            crate::log::screen(s.lines.iter().map(String::as_str));
        }
        Ok(event)
    }

    /// Back out of the picker / confirm page with Esc until neither is on the screen (each Esc
    /// only while one of them is).
    fn leave_rewind(&mut self) {
        let shown =
            |s: &Screen| rewind_picker(&s.lines).is_some() || rewind_confirm(&s.lines).is_some();
        for _ in 0..3 {
            match self.fresh_screen() {
                Ok(s) if shown(&s) => {}
                _ => return,
            }
            if self.keys(key::ESC).is_err() {
                return;
            }
            self.wait_screen(Duration::from_secs(1), |_, s| (!shown(s)).then_some(()));
        }
    }

    pub(super) fn answer_rewind(
        &mut self,
        request_id: &str,
        option: Option<RewindOption>,
    ) -> CmdResult {
        let r = match &self.rewind {
            Some(r) if r.id.as_deref() == Some(request_id) => r.clone(),
            _ => {
                return err(
                    "unknown_request",
                    format!("当前挂起的回退不是 {request_id}"),
                );
            }
        };
        match &self.state {
            State::Waiting { .. } => {}
            State::Exited => return err("exited", "Session 已结束"),
            _ => return err("not_waiting", format!("当前状态为 {}", self.state.name())),
        }
        let now = self.fresh_screen()?;
        let page = rewind_confirm(&now.lines)
            .filter(|c| r.page.as_ref().is_some_and(|p| p.same_page(c)))
            .ok_or(("unknown_request", "屏幕上已经不是这次回退".to_string()))?;
        // ccrpc drives the screens again: no announcing meanwhile, and the page leaving is ours.
        if let Some(cur) = &mut self.rewind {
            cur.id = None;
        }
        let res = self.press_rewind(&r, &page, option);
        if res.is_err() {
            self.leave_rewind();
        }
        self.rewind = None;
        res
    }

    fn press_rewind(
        &mut self,
        r: &RewindReq,
        page: &RewindConfirm,
        option: Option<RewindOption>,
    ) -> CmdResult {
        let Some(option) = option else {
            self.leave_rewind();
            return if self.wait_until(Duration::from_secs(5), |e| e.state == State::Idle) {
                Ok(json!({"state": self.state.name()}))
            } else {
                err("timeout", "取消后 5 秒内没有回到空闲")
            };
        };
        let Some(n) = page
            .options
            .iter()
            .find(|(_, l, _)| l == option.label())
            .map(|o| o.0)
        else {
            return err("bad_request", format!("这次回退没有「{}」", option.label()));
        };
        self.keys(n.to_string().as_bytes())?;
        let off = |e: &Engine| {
            e.state == State::Idle
                && e.screen().is_ok_and(|s| {
                    rewind_confirm(&s.lines).is_none() && rewind_picker(&s.lines).is_none()
                })
        };
        if !self.wait_after_keys(Duration::from_secs(10), off) {
            return err("timeout", "作答后 10 秒内没有回到空闲");
        }
        crate::clog!("rewind answered {}", option.name());
        if !option.restores_conversation() {
            // The input box was emptied before the picker opened and stays so.
            return match self.input_box_now()? {
                t if t.is_empty() => Ok(json!({"state": self.state.name(), "text": null})),
                _ => err("unrecognized_screen", "回退后输入框里多了内容"),
            };
        }
        // The TUI's conversation now ends before the prompt, which is back in the input box.
        self.rewound = Some(r.uuid.clone());
        let want = recognize::rewind_key(&r.text);
        let back = self
            .wait_screen(Duration::from_secs(3), |_, s| {
                recognize::input_text(s).filter(|t| !t.is_empty())
            })
            .map(|(t, _)| t);
        // The whole prompt, or a long enough start of it (a long one may not fit the box).
        let fits = back.as_deref().is_some_and(|t| {
            let got = recognize::rewind_key(t);
            let long_enough = got.chars().count() >= want.chars().count().min(20);
            t.contains("[Pasted text") || (long_enough && want.starts_with(&got))
        });
        if !fits {
            return err("unrecognized_screen", "回退后输入框里不是这条提示词");
        }
        Ok(json!({"state": self.state.name(), "text": r.text}))
    }

    /// While ccrpc drives the picker nothing is announced; an announced rewind ends once the
    /// Session has not been waiting on its page for [`CONFIRM_GONE`] (answered or left in the
    /// TUI), and whatever is on the screen then is reported as usual.
    pub(super) fn watch_rewind(&mut self) {
        let Some(r) = &self.rewind else {
            return;
        };
        if r.id.is_none() {
            return;
        }
        let shown = matches!(self.state, State::Waiting { .. })
            && self
                .screen()
                .ok()
                .and_then(|s| rewind_confirm(&s.lines))
                .is_some_and(|c| r.page.as_ref().is_some_and(|p| p.same_page(&c)));
        let Some(r) = &mut self.rewind else {
            return;
        };
        if shown {
            r.gone_since = None;
        } else if r.gone_since.get_or_insert_with(Instant::now).elapsed() >= CONFIRM_GONE {
            crate::clog!("rewind {:?} left in the TUI", r.id);
            self.rewind = None;
            self.raw_since = Some((
                self.raw_since.take().map(|r| r.0).unwrap_or_default(),
                Instant::now(),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(text: &str, at_ms: Option<f64>) -> Prompt {
        Prompt {
            uuid: text.to_string(),
            text: text.to_string(),
            queued: false,
            at_ms,
            command: false,
        }
    }

    fn page(quote: &[&str], ago: Option<&str>) -> RewindConfirm {
        RewindConfirm {
            quote: quote.iter().map(|q| q.to_string()).collect(),
            ago: ago.map(str::to_string),
            code: None,
            warning: None,
            options: Vec::new(),
        }
    }

    #[test]
    fn pages_are_told_apart_by_quote_and_age() {
        let now = 1_000_000_000.0;
        let ps = [
            prompt("continue", Some(now - 600_000.0)),
            prompt("fix the tests in parser", Some(now - 120_000.0)),
            prompt("fix the tests", Some(now - 60_000.0)),
            prompt("continue", Some(now - 5_000.0)),
        ];
        // A whole quote is no other prompt's start.
        assert_eq!(quoting(&ps, &page(&["fix the tests"], None)), [2]);
        assert_eq!(quoting(&ps, &page(&["continue"], None)), [0, 3]);
        // The age agrees with when the prompt was sent (a few seconds of slack), or is unknown.
        assert!(age_fits(&page(&["continue"], Some("5s ago")), &ps[3], now));
        assert!(age_fits(&page(&["continue"], Some("7s ago")), &ps[3], now));
        assert!(!age_fits(
            &page(&["continue"], Some("10m ago")),
            &ps[3],
            now
        ));
        assert!(age_fits(&page(&["continue"], Some("10m ago")), &ps[0], now));
        assert!(age_fits(&page(&["continue"], None), &ps[0], now));
        assert!(age_fits(
            &page(&["continue"], Some("2m ago")),
            &prompt("x", None),
            now
        ));
    }
}
