//! The requests reported to the caller and not ended yet: each request event goes out with its
//! `request_id`, and `request_end` goes out exactly once when the request is over, whatever ended
//! it (answered here or in the terminal, its tool ran, its dialog left the screen, the turn ended,
//! another screen took its place). Pure bookkeeping: the engine says what is live ([`Facts`]).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A main-line permission request or question is over once the Session has been out of waiting
/// this long (answered in the terminal: its tool runs, its hook still waits for PostToolUse); a
/// shorter flicker of the Process File keeps it.
pub(super) const REQUEST_AWAY: Duration = Duration::from_secs(1);

/// What the engine knows now about what is still pending.
#[derive(Debug, Default)]
pub(super) struct Facts {
    /// Permission requests and questions the plugin reported (or ccrpc adopted) not over yet.
    pub(super) open: HashSet<String>,
    /// The main line's question asked on the screen with no hook (its tool still pending).
    pub(super) screen_question: Option<String>,
    /// The main line's question still pending in the transcript, answered or not.
    pub(super) asked: Option<String>,
    /// The choice or dialog announced for the current waiting.
    pub(super) announced: Option<String>,
    /// The startup dialog, while there is no Process File yet.
    pub(super) startup_dialog: Option<String>,
    /// The rewind whose confirm page waits.
    pub(super) rewind: Option<String>,
    /// The `/btw` panel is open.
    pub(super) btw: bool,
    /// The Session waits (the Process File says so).
    pub(super) waiting: bool,
}

struct Entry {
    id: String,
    event: Value,
    /// Since when the Session has not been waiting (main-line permission requests and questions),
    /// or the dialog has not been announced (a dialog).
    away_since: Option<Instant>,
}

/// Requests reported and not ended, in the order reported.
#[derive(Default)]
pub(super) struct Reported {
    list: Vec<Entry>,
    /// Answered here: over at the next [`Reported::sync`], whatever else still holds them, and
    /// not reported again while the request is still open (its tool runs).
    answered: HashSet<String>,
}

fn end(id: &str) -> Value {
    json!({"type": "request_end", "request_id": id})
}

/// Request types that stand for what is on the screen: a newer one ends an `unrecognized` and a
/// dialog that is not the startup screen's.
const SCREEN_TYPES: &[&str] = &["dialog", "choice", "rewind", "unrecognized"];

/// A dialog of the startup screens (no Process File yet), ended only when the startup is over.
fn is_startup(e: &Entry) -> bool {
    e.event["waiting_for"] == "startup"
}

impl Reported {
    /// A request event about to go out (`request_id` set): the entry with its id is updated in
    /// place, or it is added. Returns the `request_end` events of what it ends (an
    /// `unrecognized` or a dialog once another screen is reported).
    pub(super) fn upsert(&mut self, event: Value) -> Vec<Value> {
        let Some(id) = event["request_id"].as_str().map(str::to_owned) else {
            return Vec::new();
        };
        let ty = event["type"].as_str().unwrap_or_default();
        let mut out = Vec::new();
        if SCREEN_TYPES.contains(&ty) {
            self.list.retain(|e| {
                let gone = e.id != id
                    && (e.event["type"] == "unrecognized"
                        || (e.event["type"] == "dialog" && !is_startup(e)));
                if gone {
                    out.push(end(&e.id));
                }
                !gone
            });
        }
        match self.list.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                e.event = event;
                e.away_since = None;
            }
            None => self.list.push(Entry {
                id,
                event,
                away_since: None,
            }),
        }
        out
    }

    /// `id` was answered from here (its key or hook reply went out): it ends at the next
    /// [`Reported::sync`], and is not reported again while it is open. One never reported: no
    /// effect.
    pub(super) fn answered(&mut self, id: &str) {
        if self.answered.len() > 4096 {
            self.answered.clear();
        }
        self.answered.insert(id.to_owned());
    }

    pub(super) fn is_answered(&self, id: &str) -> bool {
        self.answered.contains(id)
    }

    /// The dialog not ended yet that waits for `waiting_for` (not the startup screen's): announced
    /// again within its grace, it keeps its id.
    pub(super) fn dialog_for(&self, waiting_for: &str) -> Option<String> {
        self.list
            .iter()
            .find(|e| {
                e.event["type"] == "dialog"
                    && !is_startup(e)
                    && e.event["waiting_for"] == waiting_for
            })
            .map(|e| e.id.clone())
    }

    /// Ends every request of type `ty`.
    pub(super) fn end_type(&mut self, ty: &str) -> Vec<Value> {
        let mut out = Vec::new();
        self.list.retain(|e| {
            let gone = e.event["type"] == ty;
            if gone {
                out.push(end(&e.id));
            }
            !gone
        });
        out
    }

    /// Ends what is no longer live; returns their `request_end` events, in the order reported.
    pub(super) fn sync(&mut self, f: &Facts, now: Instant) -> Vec<Value> {
        let mut out = Vec::new();
        let answered = &self.answered;
        self.list.retain_mut(|e| {
            let live = !answered.contains(&e.id)
                && match e.event["type"].as_str().unwrap_or_default() {
                    "permission_request" | "question" => {
                        let main = e.event["agent_id"].is_null();
                        if f.waiting {
                            e.away_since = None;
                        }
                        let away = main
                            && !f.waiting
                            && now.duration_since(*e.away_since.get_or_insert(now)) >= REQUEST_AWAY;
                        (f.open.contains(&e.id) || f.screen_question.as_deref() == Some(&e.id))
                            && !away
                    }
                    "choice" => f.announced.as_deref() == Some(&e.id),
                    // The startup screen's while the startup lasts; another while announced, and
                    // for REQUEST_AWAY after (a flicker of the waiting announces it again).
                    "dialog" => {
                        if f.announced.as_deref() == Some(&e.id)
                            || f.startup_dialog.as_deref() == Some(&e.id)
                        {
                            e.away_since = None;
                            true
                        } else {
                            !is_startup(e)
                                && now.duration_since(*e.away_since.get_or_insert(now))
                                    < REQUEST_AWAY
                        }
                    }
                    "rewind" => f.rewind.as_deref() == Some(&e.id),
                    "btw" => f.btw,
                    // Ended by [`Reported::end_type`] or a newer screen ([`Reported::upsert`]).
                    _ => true,
                };
            if !live {
                out.push(end(&e.id));
            }
            live
        });
        // An answered request is not reported again until it is no longer open.
        self.answered
            .retain(|id| f.open.contains(id) || f.asked.as_deref() == Some(id));
        out
    }

    /// Forgets everything without events (the Session ended or changed: every request is over).
    pub(super) fn clear(&mut self) {
        self.list.clear();
        self.answered.clear();
    }

    /// The requests not ended, each as the event that reported it last, in the order reported.
    pub(super) fn events(&self) -> Vec<Value> {
        self.list.iter().map(|e| e.event.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission(id: &str, agent: Option<&str>) -> Value {
        json!({"type": "permission_request", "request_id": id, "tool_use_id": id,
            "agent_id": agent})
    }

    fn ends(v: &[Value]) -> Vec<&str> {
        v.iter()
            .map(|e| {
                assert_eq!(e["type"], "request_end");
                e["request_id"].as_str().unwrap()
            })
            .collect()
    }

    fn waiting_on(ids: &[&str]) -> Facts {
        Facts {
            open: ids.iter().map(|s| s.to_string()).collect(),
            waiting: true,
            ..Facts::default()
        }
    }

    #[test]
    fn a_permission_ends_once_when_its_request_is_over() {
        let now = Instant::now();
        let mut r = Reported::default();
        assert!(r.upsert(permission("toolu_1", None)).is_empty());
        assert!(r.sync(&waiting_on(&["toolu_1"]), now).is_empty());
        // Announced again (the waiting came back): updated in place, nothing ends.
        let mut again = permission("toolu_1", None);
        again["options"] = json!([]);
        assert!(r.upsert(again.clone()).is_empty());
        assert_eq!(r.events(), [again]);
        // Its tool ran (PostToolUse): over, said once.
        assert_eq!(ends(&r.sync(&waiting_on(&[]), now)), ["toolu_1"]);
        assert!(r.sync(&waiting_on(&[]), now).is_empty());
        assert!(r.events().is_empty());
    }

    #[test]
    fn a_main_line_request_answered_in_the_terminal_ends_after_a_second_away() {
        let t0 = Instant::now();
        let mut r = Reported::default();
        r.upsert(permission("main", None));
        r.upsert(permission("sub", Some("a1")));
        let away = Facts {
            waiting: false,
            ..waiting_on(&["main", "sub"])
        };
        // A flicker out of waiting keeps it.
        assert!(r.sync(&away, t0).is_empty());
        assert!(r.sync(&away, t0 + Duration::from_millis(500)).is_empty());
        assert!(
            r.sync(
                &waiting_on(&["main", "sub"]),
                t0 + Duration::from_millis(600)
            )
            .is_empty()
        );
        assert!(r.sync(&away, t0 + Duration::from_millis(700)).is_empty());
        assert!(r.sync(&away, t0 + Duration::from_millis(1600)).is_empty());
        // Away a whole second: the main line's is over; a background Subagent's waits for its tool.
        assert_eq!(
            ends(&r.sync(&away, t0 + Duration::from_millis(1700))),
            ["main"]
        );
        assert!(r.sync(&away, t0 + Duration::from_secs(10)).is_empty());
        assert_eq!(r.events().len(), 1);
    }

    /// A background Subagent's request answered by keys (no hook waits for it): it stays open until
    /// its tool has run (end to end: `mock_model_subagent_permission`).
    #[test]
    fn a_request_answered_here_ends_at_once_and_a_failed_answer_ends_nothing() {
        let now = Instant::now();
        let mut r = Reported::default();
        r.upsert(permission("sub", Some("a1")));
        r.upsert(permission("other", Some("a2")));
        let facts = waiting_on(&["sub", "other"]);
        // A failed answer marks nothing: the card stays.
        assert!(r.sync(&facts, now).is_empty());
        // Answered by keys: the request stays open until PostToolUse.
        r.answered("sub");
        assert_eq!(ends(&r.sync(&facts, now)), ["sub"]);
        assert!(r.is_answered("sub") && !r.is_answered("other"));
        assert_eq!(r.events(), [permission("other", Some("a2"))]);
        // Not reported again while open; once its tool ran the mark goes.
        assert!(r.sync(&facts, now).is_empty() && r.is_answered("sub"));
        r.sync(&waiting_on(&["other"]), now);
        assert!(!r.is_answered("sub"));
    }

    #[test]
    fn a_question_asked_on_the_screen_ends_when_its_tool_ran() {
        let now = Instant::now();
        let mut r = Reported::default();
        r.upsert(json!({"type": "question", "request_id": "toolu_q", "agent_id": null}));
        let on_screen = Facts {
            screen_question: Some("toolu_q".into()),
            waiting: true,
            ..Facts::default()
        };
        assert!(r.sync(&on_screen, now).is_empty());
        assert_eq!(ends(&r.sync(&waiting_on(&[]), now)), ["toolu_q"]);
    }

    #[test]
    fn screen_requests_end_when_the_screen_moves_on() {
        let now = Instant::now();
        let mut r = Reported::default();
        r.upsert(json!({"type": "choice", "request_id": "choice:1"}));
        r.upsert(json!({"type": "dialog", "request_id": "dialog:1", "waiting_for": "startup"}));
        r.upsert(json!({"type": "rewind", "request_id": "rewind:1"}));
        r.upsert(json!({"type": "btw", "request_id": "btw"}));
        let all = Facts {
            announced: Some("choice:1".into()),
            startup_dialog: Some("dialog:1".into()),
            rewind: Some("rewind:1".into()),
            btw: true,
            ..Facts::default()
        };
        assert!(r.sync(&all, now).is_empty());
        // A dialog announced in place of the choice.
        r.upsert(json!({"type": "dialog", "request_id": "dialog:2", "waiting_for": "dialog open"}));
        let next = Facts {
            announced: Some("dialog:2".into()),
            ..all
        };
        assert_eq!(ends(&r.sync(&next, now)), ["choice:1"]);
        // The startup is over (a Process File), the rewind's page left, the panel closed; the
        // dialog's waiting ended (over a moment later).
        assert_eq!(
            ends(&r.sync(&Facts::default(), now)),
            ["dialog:1", "rewind:1", "btw"]
        );
        assert_eq!(
            ends(&r.sync(&Facts::default(), now + REQUEST_AWAY)),
            ["dialog:2"]
        );
    }

    #[test]
    fn a_dialog_keeps_its_id_across_a_flicker_of_the_waiting() {
        let t0 = Instant::now();
        let mut r = Reported::default();
        let dialog = |id: &str, wf: &str| json!({"type": "dialog", "request_id": id, "waiting_for": wf, "lines": []});
        r.upsert(dialog("dialog:1", "dialog open"));
        let shown = Facts {
            announced: Some("dialog:1".into()),
            ..Facts::default()
        };
        assert!(r.sync(&shown, t0).is_empty());
        // The state flickered (nothing announced): kept a moment, found for its waiting_for.
        assert!(r.sync(&Facts::default(), t0).is_empty());
        assert!(
            r.sync(&Facts::default(), t0 + Duration::from_millis(900))
                .is_empty()
        );
        assert_eq!(r.dialog_for("dialog open").as_deref(), Some("dialog:1"));
        assert_eq!(r.dialog_for("permission prompt"), None);
        // Announced again with that id: updated in place, its grace starts afresh.
        assert!(r.upsert(dialog("dialog:1", "dialog open")).is_empty());
        assert!(r.sync(&shown, t0 + Duration::from_millis(950)).is_empty());
        let later = t0 + Duration::from_secs(2);
        assert!(r.sync(&Facts::default(), later).is_empty());
        // Un-announced a whole second: over.
        assert_eq!(
            ends(&r.sync(&Facts::default(), later + REQUEST_AWAY)),
            ["dialog:1"]
        );
        // Another screen taking over ends it at once; the startup screen's stays.
        r.upsert(dialog("dialog:2", "startup"));
        r.upsert(dialog("dialog:3", "dialog open"));
        assert_eq!(
            ends(&r.upsert(dialog("dialog:4", "permission prompt"))),
            ["dialog:3"]
        );
        assert_eq!(
            ends(&r.upsert(json!({"type": "choice", "request_id": "choice:1"}))),
            ["dialog:4"]
        );
        assert_eq!(r.events().len(), 2);
    }

    #[test]
    fn an_unrecognized_screen_ends_with_the_next_screen_or_by_type() {
        let now = Instant::now();
        let mut r = Reported::default();
        r.upsert(json!({"type": "unrecognized", "request_id": "unrecognized:1"}));
        // Nothing the engine knows ends it.
        assert!(r.sync(&Facts::default(), now).is_empty());
        // A permission request is no screen of its own.
        assert!(r.upsert(permission("toolu_1", None)).is_empty());
        assert_eq!(
            ends(&r.upsert(json!({"type": "unrecognized", "request_id": "unrecognized:2"}))),
            ["unrecognized:1"]
        );
        assert_eq!(
            ends(&r.upsert(json!({"type": "dialog", "request_id": "dialog:3"}))),
            ["unrecognized:2"]
        );
        // An unrecognized screen in place of the dialog ends it.
        assert_eq!(
            ends(&r.upsert(json!({"type": "unrecognized", "request_id": "unrecognized:4"}))),
            ["dialog:3"]
        );
        assert_eq!(ends(&r.end_type("unrecognized")), ["unrecognized:4"]);
        // Cleared (the Session ended): forgotten without events.
        r.clear();
        assert!(r.events().is_empty());
        assert!(r.sync(&Facts::default(), now).is_empty());
    }
}
