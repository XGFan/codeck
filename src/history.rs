//! `get_messages`: the Main Line's blocks (and the Subagent Runs') kept up to date by a worker
//! thread that reads only what was appended since the last request, so a long transcript is
//! parsed once and the engine keeps sending events meanwhile.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use serde_json::{Value, json};

use crate::transcript::{Block, Tailer, TxEvent};
use crate::wake::Wake;

/// The blocks of one transcript, as following it from the start gives them (`branch` applied).
pub struct History {
    path: PathBuf,
    tailer: Tailer,
    main: Vec<Block>,
    subs: Vec<Block>,
    /// Main file length at the last read (a shorter file was replaced: start over).
    len: u64,
}

impl History {
    pub fn new(path: &Path) -> History {
        let mut tailer = Tailer::new(path);
        tailer.keep_offsets();
        History {
            path: path.to_path_buf(),
            tailer,
            main: Vec::new(),
            subs: Vec::new(),
            len: 0,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads what was appended since the last call.
    pub fn update(&mut self) {
        let len = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if len < self.len {
            *self = History::new(&self.path.clone());
        }
        self.len = len;
        loop {
            for ev in self.tailer.poll() {
                match ev {
                    TxEvent::Block(b) if b.agent_id.is_some() => self.subs.push(*b),
                    TxEvent::Block(b) => self.main.push(*b),
                    TxEvent::Branch { at_uuid } => {
                        let keep = at_uuid
                            .and_then(|a| self.main.iter().rposition(|b| b.uuid == a))
                            .map_or(0, |i| i + 1);
                        self.main.truncate(keep);
                    }
                    _ => {}
                }
            }
            if !self.tailer.behind() {
                break;
            }
        }
    }

    /// One page: Main Line blocks written at or after `since` and before `before` (cursors are
    /// main-file offsets), the newest `limit` of them; with `subagents`, the Subagent Run blocks
    /// under the Agent / Workflow calls on the page.
    pub fn page(&self, since: u64, before: Option<u64>, limit: Option<usize>, subagents: bool) -> Value {
        let off = |b: &Block| self.tailer.offset_of(&b.uuid).unwrap_or(0);
        let range: Vec<&Block> = self
            .main
            .iter()
            .filter(|b| off(b) >= since && before.is_none_or(|x| off(b) < x))
            .collect();
        let skip = limit.map_or(0, |n| range.len().saturating_sub(n));
        let page = &range[skip..];
        let mut v = json!({
            "blocks": page,
            "cursor": self.tailer.read_to(),
            // Where the page starts: `before` for the page before it.
            "first": page.first().map(|b| off(b)),
            "more": skip > 0,
        });
        if subagents {
            let calls: std::collections::HashSet<&str> = page
                .iter()
                .filter_map(|b| b.id.as_deref())
                .collect();
            let subs: Vec<&Block> = self
                .subs
                .iter()
                .filter(|b| b.parent_tool_use_id.as_deref().is_some_and(|p| calls.contains(p)))
                .collect();
            v["subagent_blocks"] = json!(subs);
        }
        v
    }
}

pub struct Query {
    pub since: u64,
    pub before: Option<u64>,
    pub limit: Option<usize>,
    pub subagents: bool,
}

enum Req {
    /// Start reading this transcript now (nothing to answer).
    Warm(PathBuf),
    Page(PathBuf, Query, Sender<Value>),
}

/// The worker thread holding the History of the current transcript.
pub struct HistoryWorker {
    tx: Sender<Req>,
}

impl HistoryWorker {
    pub fn start(wake: Arc<Wake>) -> HistoryWorker {
        let (tx, rx) = mpsc::channel::<Req>();
        std::thread::spawn(move || {
            let mut h: Option<History> = None;
            for req in rx {
                let path = match &req {
                    Req::Warm(p) | Req::Page(p, ..) => p.clone(),
                };
                if h.as_ref().is_none_or(|h| h.path() != path) {
                    h = Some(History::new(&path));
                }
                let hist = h.as_mut().expect("set above");
                hist.update();
                if let Req::Page(_, q, reply) = req {
                    let _ = reply.send(hist.page(q.since, q.before, q.limit, q.subagents));
                    wake.bump();
                }
            }
        });
        HistoryWorker { tx }
    }

    pub fn warm(&self, path: &Path) {
        let _ = self.tx.send(Req::Warm(path.to_path_buf()));
    }

    /// Asks for a page; the answer arrives on the returned channel (the wake is bumped).
    pub fn page(&self, path: &Path, q: Query) -> Receiver<Value> {
        let (tx, rx) = mpsc::channel();
        let _ = self.tx.send(Req::Page(path.to_path_buf(), q, tx));
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn entry(uuid: &str, parent: Option<&str>, text: &str) -> String {
        json!({"type": "user", "uuid": uuid, "parentUuid": parent, "sessionId": "s",
            "message": {"role": "user", "content": text}})
        .to_string()
            + "\n"
    }

    /// Pages from the newest back, and reads only what was appended.
    #[test]
    fn pages_and_appends() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        let mut parent = None;
        for i in 0..5 {
            let u = format!("u{i}");
            f.write_all(entry(&u, parent.as_deref(), &format!("prompt {i}")).as_bytes()).unwrap();
            parent = Some(u);
        }
        let mut h = History::new(&p);
        h.update();
        let last2 = h.page(0, None, Some(2), false);
        let texts = |v: &Value| -> Vec<String> {
            v["blocks"].as_array().unwrap().iter().map(|b| b["text"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(texts(&last2), ["prompt 3", "prompt 4"]);
        assert_eq!(last2["more"], true);
        let before = last2["first"].as_u64();
        let older = h.page(0, before, Some(10), false);
        assert_eq!(texts(&older), ["prompt 0", "prompt 1", "prompt 2"]);
        assert_eq!(older["more"], false);
        let cursor = last2["cursor"].as_u64().unwrap();
        f.write_all(entry("u5", Some("u4"), "prompt 5").as_bytes()).unwrap();
        h.update();
        assert_eq!(texts(&h.page(cursor, None, None, false)), ["prompt 5"]);
        // Same view as reading the whole file afresh.
        let (fresh, _) = crate::transcript::main_blocks(&p, 0);
        assert_eq!(h.page(0, None, None, false)["blocks"], json!(fresh));
    }
}
