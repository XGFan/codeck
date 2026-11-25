//! Claude Code's input queue (main line), as the main file's `queue-operation` entries leave it.

use serde::Serialize;
use transcript::assemble::{nodes, prompt};
use transcript::model::PeerSource;
use transcript::raw::{RawEntry, decode_str};

use super::TxEvent;

/// Who a queued item is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueKind {
    /// What the person typed, or a steer (a plugin's message too: it is queued as typed).
    Typed,
    /// Another Session's message (`<cross-session-message>`).
    Session,
    /// A Subagent's or Teammate's of this Session (`<agent-message>`).
    Agent,
}

/// An item in Claude Code's queue with the number ccrpc gave it when it was enqueued.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    pub id: u64,
    pub text: String,
}

/// A queued item as the caller gets it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueItem {
    /// `q<n>`: unique within the connection, unchanged while the item waits.
    pub id: String,
    pub kind: QueueKind,
    /// The sender of a message from another Session or agent (`from-name`, else `from`).
    pub from: Option<String>,
    /// What was typed (`[Image #n]` kept); for a message, what its tags hold, without Claude
    /// Code's lines around them.
    pub text: String,
    /// `↑` takes it back into the input box: only what was typed.
    pub editable: bool,
    /// The image files of a prompt or steer sent through ccrpc, in the order of the `[Image #n]`
    /// in `text`; `[]` when it has none; null when it has some ccrpc does not know (typed in
    /// the terminal, queued before ccrpc connected).
    pub images: Option<Vec<String>>,
}

/// A queued item's text as the caller gets it. `images` is for the caller to fill in
/// (see [`QueueItem::images`]); `[]` when the text has no image tag.
pub fn queue_item(id: u64, text: &str) -> QueueItem {
    let peer = prompt::peer_text(text).and_then(|(source, messages)| match source {
        PeerSource::Session => Some((QueueKind::Session, messages)),
        PeerSource::Agent => Some((QueueKind::Agent, messages)),
        PeerSource::Plugin => None,
    });
    match peer {
        Some((kind, messages)) => QueueItem {
            id: format!("q{id}"),
            kind,
            from: messages.first().and_then(|m| m.from.clone()),
            text: messages
                .iter()
                .map(|m| m.body.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
            editable: false,
            images: image_slot(text),
        },
        None => QueueItem {
            id: format!("q{id}"),
            kind: QueueKind::Typed,
            from: None,
            text: text.to_owned(),
            editable: true,
            images: image_slot(text),
        },
    }
}

/// `[]` for a text without image tags, else unknown.
fn image_slot(text: &str) -> Option<Vec<String>> {
    (!text.contains("[Image #")).then(Vec::new)
}

/// Unconfirmed `dequeue` guesses kept ([`Queue::settle_delivered`]).
const MAX_GUESSES: usize = 8;

#[derive(Debug, Default)]
pub(super) struct Queue {
    /// What waits, oldest first, task notifications included.
    items: Vec<Queued>,
    /// The last number given: an item gets the next one when it is enqueued, and keeps it while
    /// it waits (a `dequeue` guess put back included).
    last_id: u64,
    /// Items `dequeue` was taken to have taken (the oldest of several), not yet confirmed by what
    /// was delivered ([`Queue::settle_delivered`]); the latest last.
    guesses: Vec<Queued>,
}

impl Queue {
    /// A replaced main file's new queue goes on counting where the old one stopped: a number
    /// given to the old file's item is never given again.
    pub(super) fn continue_numbering(&mut self, old: &Queue) {
        self.last_id = old.last_id;
    }

    /// What waits now, oldest first.
    pub(super) fn items(&self) -> &[Queued] {
        &self.items
    }

    /// A `queue-operation` entry of the main file.
    pub(super) fn on_operation(&mut self, e: &RawEntry, out: &mut Vec<TxEvent>) {
        let content = prompt::unwrap_pasted(&decode_str(e.content.as_deref()).unwrap_or_default());
        let before = self.items.len();
        match e.operation.as_deref() {
            Some("enqueue") => {
                self.last_id += 1;
                let id = self.last_id;
                self.items.push(Queued {
                    id,
                    text: content.clone(),
                });
                out.push(TxEvent::Enqueued { id, content });
            }
            // Absorbed into the running turn (or dropped): by its text; failing that, by
            // its text with the opening tag's attributes left out (a relayed message is
            // enqueued with a `hop-chain` attribute that its `remove` does not carry).
            // Not queued: a guess, so the `dequeue` behind it took another item, still
            // queued until what was delivered shows which.
            Some("remove") => {
                if self.unqueue(&content).is_none()
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
                let id = self.unqueue(&content).unwrap_or_else(|| {
                    self.last_id += 1;
                    self.last_id
                });
                out.push(TxEvent::Popped { id, content });
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

    /// The queued item with `content` gone (its id): by its text; failing that, by its text with
    /// the opening tag's attributes left out.
    fn unqueue(&mut self, content: &str) -> Option<u64> {
        let i = Self::find_item(&self.items, content)?;
        Some(self.items.remove(i).id)
    }

    /// Position of `content` in `items`: by its text; failing that, by its text with the opening
    /// tag's attributes left out.
    fn find_item(items: &[Queued], content: &str) -> Option<usize> {
        items
            .iter()
            .position(|q| q.text.trim() == content.trim())
            .or_else(|| {
                let key = queue_key(content);
                items.iter().position(|q| queue_key(&q.text) == key)
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
        let carries = |item: &Queued| {
            let item = item.text.trim();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_was_typed_is_taken_back() {
        let typed = |t: &str| QueueItem {
            id: "q1".into(),
            kind: QueueKind::Typed,
            from: None,
            text: t.into(),
            editable: true,
            images: Some(Vec::new()),
        };
        assert_eq!(queue_item(1, "MOCK-STEERED now"), typed("MOCK-STEERED now"));
        // Image tags: the files are not known from the text.
        assert_eq!(
            queue_item(1, "[Image #1] look"),
            QueueItem {
                images: None,
                ..typed("[Image #1] look")
            }
        );
        // A plugin's message is queued as typed.
        assert_eq!(
            queue_item(1, "The plannotator plugin sent a message:\nFeedback."),
            typed("The plannotator plugin sent a message:\nFeedback.")
        );
        // Another Session's, wrapped in Claude Code's lines, its body on several lines.
        let session = "Another Claude session sent a message:\n<cross-session-message from=\"uds:/tmp/a.sock\" from-name=\"c6\">\nhi\nthere\n</cross-session-message>\n\nThis came from another Claude session";
        assert_eq!(
            queue_item(2, session),
            QueueItem {
                id: "q2".into(),
                kind: QueueKind::Session,
                from: Some("c6".into()),
                text: "hi\nthere".into(),
                editable: false,
                images: Some(Vec::new()),
            }
        );
        assert_eq!(
            queue_item(3, "<agent-message from=\"a\">\nhi\n</agent-message>"),
            QueueItem {
                id: "q3".into(),
                kind: QueueKind::Agent,
                from: Some("a".into()),
                text: "hi".into(),
                editable: false,
                images: Some(Vec::new()),
            }
        );
        let v = serde_json::to_value(queue_item(4, "x")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"id": "q4", "kind": "typed", "from": null, "text": "x",
                "editable": true, "images": []})
        );
    }
}
