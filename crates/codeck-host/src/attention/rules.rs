//! 「已完成」与通知的规则（纯函数）：每个 Live Session 记着它现在的状态、从何时起、看过没有、通知发过没有；每条发出的
//! 通知记着它说的是哪些 Session、发到了哪一端。
//!
//! - 一轮结束（工作中 → 空闲）时没人在看它，就标「已完成」；有人看了它、或它又工作 / 等待作答，就清掉。
//! - 等待作答或已完成持续到设定的时长、期间没人看过它，到点；同一时刻到点的合成一条。每次进入这个状态只到点一次。
//! - 到点时 Mac 有人在用（或没配 Bark）发 Mac，否则推 iPhone。只发到了 Mac、还没处理完的，Mac 转为没人在用或满补推的时长
//!   后补推 iPhone，每条只补一次。
//! - 一条通知里的 Session 都看过了、状态变了或不再 Live 时，从发到过的每一端删掉。
//! - 「看」是任一端打开的就是它（桌面窗口在前台，或 iPhone 前台的 Session 页）。

use std::collections::{BTreeMap, BTreeSet};

/// Milliseconds since the Unix epoch.
pub type Ms = i64;

const MIN: Ms = 60_000;

/// What a Live Session is doing, as far as attention goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Busy,
    Waiting,
    Idle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub status: Status,
    /// When it got this status.
    pub since: Ms,
    /// Its turn ended (busy → idle) while nobody was looking, and it has not been looked at since.
    pub done: bool,
    /// Looked at since `since`.
    pub seen: bool,
    /// Its notification for this status was due (sent, or there was nowhere to send it).
    pub notified: bool,
}

pub type Tracks = BTreeMap<String, Track>;

/// The tracks after the Live Sessions' statuses `now` at `at`, `viewing` the Sessions someone looks at. Sessions no
/// longer Live are dropped.
pub fn step(
    prev: &Tracks,
    now: &BTreeMap<String, Status>,
    at: Ms,
    viewing: &BTreeSet<String>,
) -> Tracks {
    now.iter()
        .map(|(id, &status)| {
            let seen = viewing.contains(id);
            let t = match prev.get(id) {
                Some(p) if p.status == status => {
                    let mut t = p.clone();
                    if seen {
                        t.seen = true;
                        t.done = false;
                    }
                    t
                }
                p => Track {
                    status,
                    since: at,
                    done: p.is_some_and(|p| p.status == Status::Busy)
                        && status == Status::Idle
                        && !seen,
                    seen,
                    notified: false,
                },
            };
            (id.clone(), t)
        })
        .collect()
}

/// How long a Session waits or stays done unseen before its notification; `None`: none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Delays {
    pub waiting: Option<Ms>,
    pub done: Option<Ms>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Waiting,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    pub id: String,
    pub kind: Kind,
    pub since: Ms,
}

/// The notifications due at `at`, oldest first.
pub fn due(tracks: &Tracks, at: Ms, delays: &Delays) -> Vec<Due> {
    let mut out: Vec<Due> = tracks
        .iter()
        .filter(|(_, t)| !t.notified && !t.seen)
        .filter_map(|(id, t)| {
            let kind = match t.status {
                Status::Waiting => Kind::Waiting,
                Status::Idle if t.done => Kind::Done,
                _ => return None,
            };
            let delay = match kind {
                Kind::Waiting => delays.waiting,
                Kind::Done => delays.done,
            }?;
            (at - t.since >= delay).then(|| Due {
                id: id.clone(),
                kind,
                since: t.since,
            })
        })
        .collect();
    out.sort_by(|a, b| a.since.cmp(&b.since).then_with(|| a.id.cmp(&b.id)));
    out
}

/// What a notification shows about one Session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    pub due: Due,
    pub title: String,
    /// Its Project's name; `None` when unknown.
    pub project: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// The Session a click opens.
    pub session_id: String,
    /// Notification Center keeps a Session's notifications apart from the others'; `None` for one about several.
    pub thread: Option<String>,
    /// Someone is being waited for: it may come through a Focus.
    pub time_sensitive: bool,
}

/// Names a notification: a Session's own is `session-<id>`, so its next one takes this one's place; one about
/// several is `sessions-<at>`.
pub fn notice_id(items: &[Due], at: Ms) -> String {
    match items {
        [one] => format!("session-{}", one.id),
        _ => format!("sessions-{at}"),
    }
}

/// The one a click opens: the first waiting one, else the first.
fn first<T>(items: &[T], kind: impl Fn(&T) -> Kind) -> &T {
    items
        .iter()
        .find(|i| kind(i) == Kind::Waiting)
        .unwrap_or(&items[0])
}

/// One notification for everything due at once: a single Session by its title, several counted. `items` is not empty.
pub fn message(items: &[Subject], at: Ms) -> Message {
    let f = first(items, |i| i.due.kind);
    if let [one] = items {
        let what = match one.due.kind {
            Kind::Waiting => match (at - one.due.since) / MIN {
                0 => "等待作答".to_string(),
                mins => format!("等待作答 {mins} 分钟"),
            },
            Kind::Done => "已完成".to_string(),
        };
        return Message {
            title: one.title.clone(),
            body: match &one.project {
                Some(p) => format!("{what} · {p}"),
                None => what,
            },
            session_id: one.due.id.clone(),
            thread: Some(one.due.id.clone()),
            time_sensitive: one.due.kind == Kind::Waiting,
        };
    }
    let waiting = items.iter().filter(|i| i.due.kind == Kind::Waiting).count();
    let done = items.len() - waiting;
    let body = [
        (waiting > 0).then(|| format!("{waiting} 个等待作答")),
        (done > 0).then(|| format!("{done} 个已完成")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("，");
    Message {
        title: format!("{} 个 Session 需要处理", items.len()),
        body,
        session_id: f.due.id.clone(),
        thread: None,
        time_sensitive: waiting > 0,
    }
}

/// Where a notification goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Mac,
    Phone,
}

/// A notification that went out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    /// Its id (each end finds it by that).
    pub id: String,
    pub sessions: Vec<String>,
    pub at: Ms,
    /// When it went to the Mac.
    pub mac: Option<Ms>,
    /// When it went to the iPhone.
    pub phone: Option<Ms>,
}

/// Where notifications can go now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channels {
    /// The Mac's notifications: `None` on a Host without them; else whether someone uses the Mac now.
    pub mac: Option<bool>,
    /// Bark is set up.
    pub phone: bool,
    /// A notification only on the Mac goes to the iPhone too after this long unsettled; `None`: only once nobody
    /// uses the Mac.
    pub escalate: Option<Ms>,
}

impl Channels {
    /// Where one due now goes.
    fn target(&self) -> Option<Target> {
        match self.mac {
            Some(true) => Some(Target::Mac),
            Some(false) if !self.phone => Some(Target::Mac),
            _ if self.phone => Some(Target::Phone),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Show this notification at `to`, in place of the one with its `id` there.
    Send {
        id: String,
        items: Vec<Due>,
        to: Target,
    },
    /// Take this notification away from `to`.
    Remove { id: String, to: Target },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub tracks: Tracks,
    /// Notifications sent this run that may still be shown.
    pub sent: Vec<Sent>,
}

impl State {
    /// The Sessions shown as 「已完成」.
    pub fn done(&self) -> Vec<String> {
        self.tracks
            .iter()
            .filter(|(_, t)| t.done)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// The Sessions of `s` still to tell about: not looked at, same status since it went out.
fn pending(s: &Sent, tracks: &Tracks) -> Vec<Due> {
    s.sessions
        .iter()
        .filter_map(|id| {
            let t = tracks.get(id)?;
            (t.since <= s.at && !t.seen).then(|| Due {
                id: id.clone(),
                kind: if t.status == Status::Waiting {
                    Kind::Waiting
                } else {
                    Kind::Done
                },
                since: t.since,
            })
        })
        .collect()
}

/// One step at `at`: the state after the Live Sessions' `statuses`, and what to show or take away.
pub fn advance(
    prev: &State,
    statuses: &BTreeMap<String, Status>,
    at: Ms,
    viewing: &BTreeSet<String>,
    delays: &Delays,
    channels: &Channels,
) -> (State, Vec<Action>) {
    let mut tracks = step(&prev.tracks, statuses, at, viewing);
    let mut sent = prev.sent.clone();
    let mut actions = Vec::new();

    let fire = due(&tracks, at, delays);
    if !fire.is_empty() {
        for f in &fire {
            if let Some(t) = tracks.get_mut(&f.id) {
                t.notified = true;
            }
        }
        if let Some(to) = channels.target() {
            let id = notice_id(&fire, at);
            // An earlier one with this id is no longer there where this one goes; elsewhere it is taken away.
            if let Some(old) = sent.iter().position(|s| s.id == id) {
                let old = sent.remove(old);
                if old.mac.is_some() && to != Target::Mac {
                    actions.push(Action::Remove {
                        id: id.clone(),
                        to: Target::Mac,
                    });
                }
                if old.phone.is_some() && to != Target::Phone {
                    actions.push(Action::Remove {
                        id: id.clone(),
                        to: Target::Phone,
                    });
                }
            }
            sent.push(Sent {
                id: id.clone(),
                sessions: fire.iter().map(|f| f.id.clone()).collect(),
                at,
                mac: (to == Target::Mac).then_some(at),
                phone: (to == Target::Phone).then_some(at),
            });
            actions.push(Action::Send {
                id,
                items: fire,
                to,
            });
        }
    }

    if channels.phone {
        let away = channels.mac == Some(false);
        for s in sent.iter_mut() {
            let Some(mac_at) = s.mac else { continue };
            if s.phone.is_some() || !(away || channels.escalate.is_some_and(|d| at - mac_at >= d)) {
                continue;
            }
            let items = pending(s, &tracks);
            if items.is_empty() {
                continue;
            }
            s.phone = Some(at);
            actions.push(Action::Send {
                id: s.id.clone(),
                items,
                to: Target::Phone,
            });
        }
    }

    sent.retain(|s| {
        if !pending(s, &tracks).is_empty() {
            return true;
        }
        for (when, to) in [(s.mac, Target::Mac), (s.phone, Target::Phone)] {
            if when.is_some() {
                actions.push(Action::Remove {
                    id: s.id.clone(),
                    to,
                });
            }
        }
        false
    });
    (State { tracks, sent }, actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAYS: Delays = Delays {
        waiting: Some(3 * MIN),
        done: Some(3 * MIN),
    };

    fn st(pairs: &[(&str, Status)]) -> BTreeMap<String, Status> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn view(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    use Status::{Busy, Idle, Waiting};

    #[test]
    fn a_turn_that_ended_unseen_is_done_until_looked_at() {
        let t = step(&Tracks::new(), &st(&[("a", Busy)]), 0, &view(&[]));
        let t = step(&t, &st(&[("a", Idle)]), 10, &view(&["b"]));
        assert_eq!(
            t["a"],
            Track {
                status: Idle,
                since: 10,
                done: true,
                seen: false,
                notified: false
            }
        );
        let t = step(&t, &st(&[("a", Idle)]), 20, &view(&["a"]));
        assert!(!t["a"].done && t["a"].seen);
    }

    #[test]
    fn no_done_for_a_turn_seen_ending_nor_for_idle_that_was_no_turn() {
        let seen = step(
            &step(&Tracks::new(), &st(&[("a", Busy)]), 0, &view(&[])),
            &st(&[("a", Idle)]),
            10,
            &view(&["a"]),
        );
        assert!(!seen["a"].done);
        let answered = step(
            &step(&Tracks::new(), &st(&[("a", Waiting)]), 0, &view(&[])),
            &st(&[("a", Idle)]),
            10,
            &view(&[]),
        );
        assert!(!answered["a"].done);
        assert!(!step(&Tracks::new(), &st(&[("a", Idle)]), 0, &view(&[]))["a"].done);
    }

    #[test]
    fn done_clears_when_it_works_again_and_sessions_no_longer_live_go() {
        let t = step(
            &step(
                &Tracks::new(),
                &st(&[("a", Busy), ("b", Busy)]),
                0,
                &view(&[]),
            ),
            &st(&[("a", Idle), ("b", Idle)]),
            10,
            &view(&[]),
        );
        let t = step(&t, &st(&[("a", Busy)]), 20, &view(&[]));
        assert!(!t["a"].done);
        assert!(!t.contains_key("b"));
    }

    #[test]
    fn due_after_the_delay_once_unless_seen_or_off() {
        let waiting = step(&Tracks::new(), &st(&[("w", Waiting)]), 0, &view(&[]));
        assert!(due(&waiting, 3 * MIN - 1, &DELAYS).is_empty());
        assert_eq!(
            due(&waiting, 3 * MIN, &DELAYS),
            [Due {
                id: "w".into(),
                kind: Kind::Waiting,
                since: 0
            }]
        );
        let mut told = waiting.clone();
        told.get_mut("w").unwrap().notified = true;
        assert!(due(&told, 4 * MIN, &DELAYS).is_empty());
        let looked = step(&waiting, &st(&[("w", Waiting)]), MIN, &view(&["w"]));
        assert!(due(&looked, 5 * MIN, &DELAYS).is_empty());
        let off = Delays {
            waiting: None,
            ..DELAYS
        };
        assert!(due(&waiting, 5 * MIN, &off).is_empty());
        let now = Delays {
            waiting: Some(0),
            done: Some(0),
        };
        assert_eq!(due(&waiting, 0, &now).len(), 1);
    }

    #[test]
    fn done_counts_from_the_end_of_the_turn_and_starts_over_on_a_new_status() {
        let t = step(
            &step(&Tracks::new(), &st(&[("d", Busy)]), 0, &view(&[])),
            &st(&[("d", Idle)]),
            MIN,
            &view(&[]),
        );
        assert!(due(&t, 4 * MIN - 1, &DELAYS).is_empty());
        assert_eq!(due(&t, 4 * MIN, &DELAYS)[0].kind, Kind::Done);
        let t = step(&t, &st(&[("d", Waiting)]), 2 * MIN, &view(&[]));
        assert!(due(&t, 4 * MIN, &DELAYS).is_empty());
    }

    fn subject(id: &str, kind: Kind, since: Ms, title: &str, project: Option<&str>) -> Subject {
        Subject {
            due: Due {
                id: id.into(),
                kind,
                since,
            },
            title: title.into(),
            project: project.map(Into::into),
        }
    }

    #[test]
    fn a_message_names_one_session_with_how_long_and_its_project() {
        let one = [subject("a", Kind::Waiting, 0, "修登录", Some("codeck"))];
        assert_eq!(
            message(&one, 3 * MIN + 5000),
            Message {
                title: "修登录".into(),
                body: "等待作答 3 分钟 · codeck".into(),
                session_id: "a".into(),
                thread: Some("a".into()),
                time_sensitive: true,
            }
        );
        assert_eq!(notice_id(&[one[0].due.clone()], 0), "session-a");
        let done = message(&[subject("a", Kind::Done, 0, "修登录", None)], 0);
        assert_eq!((done.body.as_str(), done.time_sensitive), ("已完成", false));
    }

    #[test]
    fn a_message_counts_several_and_opens_the_first_waiting_one() {
        let items = [
            subject("d", Kind::Done, 0, "x", None),
            subject("w", Kind::Waiting, 5, "y", None),
            subject("w2", Kind::Waiting, 6, "z", None),
        ];
        assert_eq!(
            message(&items, MIN),
            Message {
                title: "3 个 Session 需要处理".into(),
                body: "2 个等待作答，1 个已完成".into(),
                session_id: "w".into(),
                thread: None,
                time_sensitive: true,
            }
        );
        let dues: Vec<Due> = items.iter().map(|i| i.due.clone()).collect();
        assert_eq!(notice_id(&dues, MIN), format!("sessions-{MIN}"));
    }

    const MAC_IN_USE: Channels = Channels {
        mac: Some(true),
        phone: true,
        escalate: Some(5 * MIN),
    };

    /// Runs `advance` from `state` and returns the new state with its actions.
    fn run(
        state: &State,
        statuses: &[(&str, Status)],
        at: Ms,
        viewing: &[&str],
        ch: Channels,
    ) -> (State, Vec<Action>) {
        advance(state, &st(statuses), at, &view(viewing), &DELAYS, &ch)
    }

    fn send(id: &str, items: &[&str], to: Target) -> Action {
        Action::Send {
            id: id.into(),
            items: items
                .iter()
                .map(|i| Due {
                    id: i.to_string(),
                    kind: Kind::Waiting,
                    since: 0,
                })
                .collect(),
            to,
        }
    }

    fn remove(id: &str, to: Target) -> Action {
        Action::Remove { id: id.into(), to }
    }

    /// `w` has been waiting since 0.
    fn waiting_w() -> State {
        run(&State::default(), &[("w", Waiting)], 0, &[], MAC_IN_USE).0
    }

    #[test]
    fn due_goes_to_the_mac_while_someone_uses_it_else_to_the_phone() {
        let (_, a) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], MAC_IN_USE);
        assert_eq!(a, [send("session-w", &["w"], Target::Mac)]);
        let away = Channels {
            mac: Some(false),
            ..MAC_IN_USE
        };
        let (_, a) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], away);
        assert_eq!(a, [send("session-w", &["w"], Target::Phone)]);
        // Without Bark the Mac it is, used or not; a Host without the Mac's notifications pushes or says nothing.
        let no_bark = Channels {
            mac: Some(false),
            phone: false,
            escalate: None,
        };
        let (_, a) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], no_bark);
        assert_eq!(a, [send("session-w", &["w"], Target::Mac)]);
        let daemon = Channels {
            mac: None,
            phone: true,
            escalate: None,
        };
        let (_, a) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], daemon);
        assert_eq!(a, [send("session-w", &["w"], Target::Phone)]);
        let nothing = Channels::default();
        let (s, a) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], nothing);
        assert!(a.is_empty());
        assert!(s.sent.is_empty() && s.tracks["w"].notified);
    }

    #[test]
    fn one_only_on_the_mac_goes_to_the_phone_once_nobody_uses_the_mac() {
        let (s, _) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], MAC_IN_USE);
        let (s, a) = run(&s, &[("w", Waiting)], 4 * MIN, &[], MAC_IN_USE);
        assert!(a.is_empty());
        let away = Channels {
            mac: Some(false),
            ..MAC_IN_USE
        };
        let (s, a) = run(&s, &[("w", Waiting)], 4 * MIN, &[], away);
        assert_eq!(a, [send("session-w", &["w"], Target::Phone)]);
        // Once.
        let (_, a) = run(&s, &[("w", Waiting)], 20 * MIN, &[], away);
        assert!(a.is_empty());
    }

    #[test]
    fn one_only_on_the_mac_goes_to_the_phone_after_the_escalation_delay() {
        let (s, _) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], MAC_IN_USE);
        let (s, a) = run(&s, &[("w", Waiting)], 8 * MIN - 1, &[], MAC_IN_USE);
        assert!(a.is_empty());
        let (s, a) = run(&s, &[("w", Waiting)], 8 * MIN, &[], MAC_IN_USE);
        assert_eq!(a, [send("session-w", &["w"], Target::Phone)]);
        assert_eq!(s.sent[0].phone, Some(8 * MIN));
        // Escalation off: only leaving the Mac sends it on.
        let off = Channels {
            escalate: None,
            ..MAC_IN_USE
        };
        let (s, _) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], off);
        let (_, a) = run(&s, &[("w", Waiting)], 60 * MIN, &[], off);
        assert!(a.is_empty());
        // Without Bark nothing goes on.
        let no_bark = Channels {
            phone: false,
            ..MAC_IN_USE
        };
        let (s, _) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], no_bark);
        let (_, a) = run(
            &s,
            &[("w", Waiting)],
            60 * MIN,
            &[],
            Channels {
                mac: Some(false),
                ..no_bark
            },
        );
        assert!(a.is_empty());
    }

    #[test]
    fn escalation_tells_only_the_sessions_still_pending() {
        let both = run(
            &State::default(),
            &[("a", Waiting), ("b", Waiting)],
            0,
            &[],
            MAC_IN_USE,
        )
        .0;
        let (s, a) = run(
            &both,
            &[("a", Waiting), ("b", Waiting)],
            3 * MIN,
            &[],
            MAC_IN_USE,
        );
        let id = format!("sessions-{}", 3 * MIN);
        assert_eq!(a, [send(&id, &["a", "b"], Target::Mac)]);
        // `a` is answered; the phone hears of `b` alone, under the same id.
        let (_, a) = run(&s, &[("a", Busy), ("b", Waiting)], 8 * MIN, &[], MAC_IN_USE);
        assert_eq!(a, [send(&id, &["b"], Target::Phone)]);
    }

    #[test]
    fn looked_at_anywhere_it_goes_from_every_end_it_went_to() {
        let away = Channels {
            mac: Some(false),
            ..MAC_IN_USE
        };
        let (s, _) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], MAC_IN_USE);
        let (s, _) = run(&s, &[("w", Waiting)], 4 * MIN, &[], away);
        assert_eq!(s.sent[0].phone, Some(4 * MIN));
        // Opened on the phone: the Mac is not told again, and both take theirs away.
        let (s, a) = run(&s, &[("w", Waiting)], 5 * MIN, &["w"], MAC_IN_USE);
        assert_eq!(
            a,
            [
                remove("session-w", Target::Mac),
                remove("session-w", Target::Phone)
            ]
        );
        assert!(s.sent.is_empty());
        // Answered on the Mac: one only on the phone goes from the phone.
        let (s, _) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], away);
        let (_, a) = run(&s, &[("w", Busy)], 4 * MIN, &[], MAC_IN_USE);
        assert_eq!(a, [remove("session-w", Target::Phone)]);
        // No longer Live.
        let (s, _) = run(&waiting_w(), &[("w", Waiting)], 3 * MIN, &[], MAC_IN_USE);
        let (_, a) = run(&s, &[], 4 * MIN, &[], MAC_IN_USE);
        assert_eq!(a, [remove("session-w", Target::Mac)]);
    }

    #[test]
    fn several_stay_until_each_is_settled() {
        let both = run(
            &State::default(),
            &[("a", Waiting), ("b", Waiting)],
            0,
            &[],
            MAC_IN_USE,
        )
        .0;
        let (s, _) = run(
            &both,
            &[("a", Waiting), ("b", Waiting)],
            3 * MIN,
            &[],
            MAC_IN_USE,
        );
        let (s, a) = run(
            &s,
            &[("a", Waiting), ("b", Waiting)],
            4 * MIN,
            &["a"],
            MAC_IN_USE,
        );
        assert!(a.is_empty());
        let (_, a) = run(
            &s,
            &[("a", Waiting), ("b", Busy)],
            4 * MIN,
            &["a"],
            MAC_IN_USE,
        );
        assert_eq!(a, [remove(&format!("sessions-{}", 3 * MIN), Target::Mac)]);
    }

    #[test]
    fn a_sessions_next_notification_takes_the_place_of_its_last() {
        let away = Channels {
            mac: Some(false),
            ..MAC_IN_USE
        };
        let now = Delays {
            waiting: Some(0),
            done: Some(0),
        };
        // Done at 5 and pushed to the phone; at 20 it asks something and the Mac is in use: told there under the
        // same id, and the phone's goes.
        let s = advance(
            &State::default(),
            &st(&[("w", Busy)]),
            0,
            &view(&[]),
            &now,
            &away,
        )
        .0;
        let (s, a) = advance(&s, &st(&[("w", Idle)]), 5, &view(&[]), &now, &away);
        assert_eq!(a.len(), 1);
        assert_eq!(s.done(), ["w"]);
        let (s, a) = advance(
            &s,
            &st(&[("w", Waiting)]),
            20,
            &view(&[]),
            &now,
            &MAC_IN_USE,
        );
        assert_eq!(
            a,
            [
                remove("session-w", Target::Phone),
                Action::Send {
                    id: "session-w".into(),
                    items: vec![Due {
                        id: "w".into(),
                        kind: Kind::Waiting,
                        since: 20
                    }],
                    to: Target::Mac
                }
            ]
        );
        assert_eq!(s.sent.len(), 1);
        assert!(s.done().is_empty());
    }
}
