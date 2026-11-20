//! Claude Code's input queue (main line), as the main file's `queue-operation` entries leave it.

use ::transcript::assemble::{nodes, prompt};
use ::transcript::raw::{RawEntry, decode_str};

use super::TxEvent;

/// Unconfirmed `dequeue` guesses kept ([`Queue::settle_delivered`]).
const MAX_GUESSES: usize = 8;

#[derive(Debug, Default)]
pub(super) struct Queue {
    /// What waits, oldest first, task notifications included.
    items: Vec<String>,
    /// Items `dequeue` was taken to have taken (the oldest of several), not yet confirmed by what
    /// was delivered ([`Queue::settle_delivered`]); the latest last.
    guesses: Vec<String>,
}

impl Queue {
    /// What waits now, oldest first.
    pub(super) fn items(&self) -> &[String] {
        &self.items
    }

    /// A `queue-operation` entry of the main file.
    pub(super) fn on_operation(&mut self, e: &RawEntry, out: &mut Vec<TxEvent>) {
        let content = prompt::unwrap_pasted(&decode_str(e.content.as_deref()).unwrap_or_default());
        let before = self.items.len();
        match e.operation.as_deref() {
            Some("enqueue") => {
                self.items.push(content.clone());
                out.push(TxEvent::Enqueued { content });
            }
            // Absorbed into the running turn (or dropped): by its text; failing that, by
            // its text with the opening tag's attributes left out (a relayed message is
            // enqueued with a `hop-chain` attribute that its `remove` does not carry).
            // Not queued: a guess, so the `dequeue` behind it took another item, still
            // queued until what was delivered shows which.
            Some("remove") => {
                if !self.unqueue(&content)
                    && let Some(i) = Self::find_item(&self.guesses, &content)
                {
                    self.guesses.remove(i);
                }
            }
            // Taken to run: the oldest, unless what is delivered shows otherwise.
            Some("dequeue") if !self.items.is_empty() => {
                let oldest = self.items.remove(0);
                if !self.items.is_empty() {
                    self.guesses.push(oldest);
                    if self.guesses.len() > MAX_GUESSES {
                        self.guesses.remove(0);
                    }
                }
            }
            // `↑` in the TUI takes what can be edited back into the input box, one record
            // per item (2.1.318); one without its text took all of it.
            Some("popAll") if e.content.is_some() => {
                self.unqueue(&content);
                out.push(TxEvent::Popped { content });
            }
            Some("popAll") => self.items.clear(),
            _ => {}
        }
        // The queue is never short of Claude Code's, so once it is empty no guess is owed.
        if self.items.is_empty() {
            self.guesses.clear();
        }
        let changed = self.items.len() != before || e.operation.as_deref() == Some("enqueue");
        if changed {
            out.push(TxEvent::Queue(self.items.clone()));
        }
    }

    /// The queued item with `content` gone: by its text; failing that, by its text with the
    /// opening tag's attributes left out.
    fn unqueue(&mut self, content: &str) -> bool {
        let Some(i) = Self::find_item(&self.items, content) else {
            return false;
        };
        self.items.remove(i);
        true
    }

    /// Position of `content` in `items`: by its text; failing that, by its text with the opening
    /// tag's attributes left out.
    fn find_item(items: &[String], content: &str) -> Option<usize> {
        items
            .iter()
            .position(|q| q.trim() == content.trim())
            .or_else(|| {
                let key = queue_key(content);
                items.iter().position(|q| queue_key(q) == key)
            })
    }

    /// A `dequeue` names nothing, so with more than one item queued it is first taken to have taken
    /// the oldest, kept as a guess. A Poll takes the harness event it waits for, which need not be
    /// the oldest (older ones are `remove`d later), and the entry that shows which one left may come
    /// after other queue operations. So what was delivered settles it: a user entry (main line or
    /// a Subagent Run, whose code is handed its events through the same queue) that carries a
    /// queued item (the prompt, or the Poll result that delivered an event) means that
    /// item left; when it was not the guess, the latest guess goes back to the front. A carried
    /// guess is confirmed; a `remove` of a guess shows it was wrong (see the `queue-operation`
    /// arm). A queued tag (a harness event, a notification) counts when a tool result contains
    /// it; a plain prompt only as a prompt entry of its own, since a tool output may happen to
    /// contain its words. True when the queue changed.
    pub(super) fn settle_delivered(&mut self, e: &RawEntry) -> bool {
        if self.items.is_empty() && self.guesses.is_empty() {
            return false;
        }
        let results: Vec<String> = e
            .blocks()
            .iter()
            .filter(|b| b.block_type() == "tool_result")
            .map(|b| nodes::result_content(b.content.as_deref()).0)
            .collect();
        let prompt = results.is_empty().then(|| prompt::user_text(e));
        let carries = |item: &str| {
            let item = item.trim();
            !item.is_empty()
                && (prompt.as_deref().is_some_and(|p| p.trim() == item)
                    || (item.starts_with('<') && results.iter().any(|r| r.contains(item))))
        };
        self.guesses.retain(|g| !carries(g));
        let before = self.items.len();
        self.items.retain(|q| !carries(q));
        let left = before - self.items.len();
        for _ in 0..left {
            let Some(g) = self.guesses.pop() else { break };
            self.items.insert(0, g);
        }
        left > 0
    }
}

/// A queued item's text with the attributes of a leading tag on its own line left out:
/// `<cross-session-message from="…" hop-chain="…">` → `<cross-session-message>`.
fn queue_key(text: &str) -> String {
    let t = text.trim();
    if let Some((first, rest)) = t.split_once('\n')
        && let Some(open) = first.strip_prefix('<')
        && first.ends_with('>')
    {
        let name: String = open
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
            .collect();
        if !name.is_empty() {
            return format!("<{name}>\n{rest}");
        }
    }
    t.to_owned()
}
