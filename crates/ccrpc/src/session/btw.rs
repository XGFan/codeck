//! The `/btw` side question: Claude Code answers in a panel in place of the input box and writes
//! nothing to the transcript, so the panel is the only place to read the answer from. The answer
//! goes out as a `btw` event, `btw_closed` when the panel is gone.

use super::*;

/// An answer taller than the panel: a `↓` that moved nothing within this long was the last.
const SCROLL_SETTLE: Duration = Duration::from_millis(500);
/// Off the screen this long: closed (the footer is replaced for a moment by a key's hint).
const GONE_AFTER: Duration = Duration::from_millis(500);
/// `↓` scrolls 3 rows: an answer longer than this is cut.
const MAX_SCROLLS: u32 = 200;

/// The panel as last seen.
#[derive(Default)]
pub(super) struct BtwWatch {
    screen_seen: Option<u64>,
    /// The panel on screen, once announced or being read.
    question: Option<String>,
    answered: bool,
    read: Option<Read>,
    gone_since: Option<Instant>,
}

/// Reading an answer by scrolling through it.
struct Read {
    rows: Vec<String>,
    view: Vec<String>,
    pressed_at: Instant,
    presses: u32,
}

/// `/btw` first in what is typed: answered in the panel, never in the transcript.
pub(super) fn is_btw(text: &str) -> bool {
    let t = text.trim_start();
    t.strip_prefix("/btw")
        .is_some_and(|r| r.is_empty() || r.starts_with(char::is_whitespace))
}

impl Engine {
    pub(super) fn btw_open(&self) -> bool {
        self.screen()
            .is_ok_and(|s| recognize::btw_panel(&s.lines).is_some())
    }

    fn emit_btw(&self, question: &str, text: Option<String>) {
        self.emit(
            json!({"type": "btw", "request_id": "btw", "question": question,
            "answering": text.is_none(), "text": text}),
        );
    }

    /// Follows the panel, whoever opened it.
    pub(super) fn see_btw(&mut self) {
        let generation = self.conn.generation();
        if self.btw.screen_seen == Some(generation)
            && self.btw.read.is_none()
            && self.btw.gone_since.is_none()
        {
            return;
        }
        self.btw.screen_seen = Some(generation);
        let Ok(s) = self.screen() else { return };
        let Some(panel) = recognize::btw_panel(&s.lines) else {
            if self.btw.question.is_some()
                && self
                    .btw
                    .gone_since
                    .get_or_insert_with(Instant::now)
                    .elapsed()
                    >= GONE_AFTER
            {
                self.btw = BtwWatch::default();
                self.emit(json!({"type": "btw_closed"}));
            }
            return;
        };
        self.btw.gone_since = None;
        if self.btw.question.as_deref() != Some(panel.question.as_str()) {
            crate::clog!("recognize /btw panel");
            crate::log::screen(s.lines.iter().map(String::as_str));
            self.btw.question = Some(panel.question.clone());
            self.btw.answered = false;
            self.btw.read = None;
            if panel.answering {
                self.emit_btw(&panel.question, None);
            }
        }
        if panel.answering || self.btw.answered {
            return;
        }
        let mut read = self.btw.read.take().unwrap_or_else(|| Read {
            rows: panel.rows.clone(),
            view: panel.rows.clone(),
            pressed_at: Instant::now(),
            presses: 0,
        });
        let moved = read.view != panel.rows;
        if moved {
            recognize::btw_merge(&mut read.rows, &panel.rows);
            read.view = panel.rows.clone();
        }
        let settled = read.presses > 0 && read.pressed_at.elapsed() >= SCROLL_SETTLE;
        if read.presses >= MAX_SCROLLS || (settled && !moved) {
            // Back to the top, as the person watching left it.
            let up = key::UP.repeat(read.presses as usize);
            if let Err((_, e)) = self.keys(&up) {
                crate::clog!("/btw panel: {e}");
            }
            self.btw.answered = true;
            self.emit_btw(&panel.question, Some(recognize::btw_text(&read.rows)));
            return;
        }
        if read.presses == 0 || moved {
            if let Err((_, e)) = self.keys(key::DOWN) {
                crate::clog!("/btw panel: {e}");
            }
            read.presses += 1;
            read.pressed_at = Instant::now();
        }
        self.btw.read = Some(read);
    }

    /// Types a `/btw`, confirmed by its panel: nothing of it reaches the transcript.
    pub(super) fn send_btw(&mut self, clean: &str, images: &[String]) -> CmdResult {
        if !images.is_empty() {
            return err("bad_request", "以 / 开头的提示词不能带图片");
        }
        self.clear_input()?;
        self.paste_and_submit(clean)?;
        let shown = self.wait_screen(Duration::from_secs(5), |_, s| {
            recognize::btw_panel(&s.lines).map(|_| ())
        });
        match shown {
            Some(_) => Ok(json!({"delivered_as": "btw"})),
            None => err("timeout", "5 秒内没有出现 /btw 面板"),
        }
    }

    /// Esc on the panel, only while it is on screen: without it, Esc would stop the turn.
    pub(super) fn close_btw(&mut self) -> CmdResult {
        let open = recognize::btw_panel(&self.fresh_screen()?.lines).is_some();
        if !open {
            return err("not_found", "没有开着的 /btw 面板");
        }
        // No more scrolling: the keys would land in the input box once the panel is gone.
        self.btw.read = None;
        self.btw.answered = true;
        self.keys(key::ESC)?;
        let closed = self.wait_screen(Duration::from_secs(3), |_, s| {
            recognize::btw_panel(&s.lines).is_none().then_some(())
        });
        match closed {
            Some(_) => Ok(json!({})),
            None => err("timeout", "/btw 面板没有关上"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_btw;

    #[test]
    fn only_the_command_itself_counts() {
        assert!(is_btw("/btw why"));
        assert!(is_btw("  /btw\nwhy"));
        assert!(is_btw("/btw"));
        assert!(!is_btw("/btwx why"));
        assert!(!is_btw("see /btw"));
    }
}
