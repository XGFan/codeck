//! Branches of the main file: whether a prompt moved the Main Line (rewind, edit and resend), and
//! which blocks already reported fell off it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use transcript::assemble::{LoadedFile, prompt, skeleton, tree};
use transcript::model::FileRole;
use transcript::raw::RawContent;

use super::TxEvent;
use super::block::BlockKind;
use super::read::Line;

/// What a main-file entry that produced blocks was, for the resend rule of Branches.
#[derive(Debug, Clone, PartialEq)]
enum Emitted {
    Prompt(String),
    /// `[Request interrupted by user…]`.
    Interruption,
    Other,
}

#[derive(Debug, Default)]
pub(super) struct Branches {
    /// Main-file entries that produced blocks, in order.
    emitted: Vec<(String, Emitted)>,
    emitted_set: HashSet<String>,
    /// Parent of every main-file entry read so far (`logicalParentUuid` across a compact).
    parents: HashMap<String, Option<String>>,
}

impl Branches {
    /// A user prompt may start a new Branch (rewind, edit and resend). Its ancestors are walked
    /// past entries that produced no block (attachments, system entries, hooks); reaching the
    /// last entry that did means it simply continues the conversation. Reaching an earlier one,
    /// or the root, makes it a candidate checked against the Main Line. An ancestor not read yet
    /// (Claude Code sometimes writes a child before its parent) means no verdict: not a fork.
    pub(super) fn is_fork(&self, line: &Line) -> bool {
        // Only a person's prompt: background results and teammate messages hang off where they
        // were started, not off the end of the conversation.
        if prompt_text(line).is_none() || !prompt::is_human_prompt(&line.entry) {
            return false;
        }
        let Some(last) = self.emitted.last().map(|(u, _)| u) else {
            return false;
        };
        let e = &line.entry;
        let mut cur = e
            .parent_uuid
            .clone()
            .or_else(|| e.logical_parent_uuid.clone());
        for _ in 0..=self.parents.len() {
            let Some(u) = cur else {
                return true;
            };
            if self.emitted_set.contains(&u) {
                return &u != last;
            }
            match self.parents.get(&u) {
                Some(p) => cur = p.clone(),
                None => return false,
            }
        }
        false
    }

    /// Records the main-file entry behind blocks just produced (for Branch checks).
    pub(super) fn note_emitted(&mut self, line: &Line, new: &[TxEvent]) {
        let e = &line.entry;
        let Some(uuid) = e.uuid.clone() else {
            return;
        };
        self.parents.insert(
            uuid.clone(),
            e.parent_uuid
                .clone()
                .or_else(|| e.logical_parent_uuid.clone()),
        );
        let mut kind = Emitted::Other;
        let mut any = false;
        for ev in new {
            match ev {
                TxEvent::Block(b) if b.agent_id.is_none() => {
                    any = true;
                    if b.kind == BlockKind::User
                        && b.text.as_deref().is_some_and(prompt::is_interruption)
                    {
                        kind = Emitted::Interruption;
                    }
                }
                TxEvent::UserPrompt { text, .. } => kind = Emitted::Prompt(text.clone()),
                _ => {}
            }
        }
        if any && self.emitted_set.insert(uuid.clone()) {
            self.emitted.push((uuid, kind));
        }
    }

    /// With the Main Line going through `fork`, the blocks already emitted that fell off it:
    /// `Some(at_uuid)` to report, `at_uuid` being the last emitted block entry still on the Main
    /// Line (`None`: none is). They are dropped from the record either way. A Branch that only
    /// resent the same prompt with no reply, an interruption note aside, is not reported. Only the
    /// file up to `fork` counts, so the verdict does not depend on when the file was read.
    pub(super) fn moved(&mut self, main: &Path, fork: &Line) -> Option<Option<String>> {
        let next_prompt = prompt_text(fork);
        let next_prompt = next_prompt.as_deref();
        let abandoned = off_main_uuids(main, fork.entry.uuid.as_deref()?, fork.offset)?;
        let off: Vec<Emitted> = self
            .emitted
            .iter()
            .filter(|(u, _)| abandoned.contains(u))
            .map(|(_, k)| k.clone())
            .collect();
        if off.is_empty() {
            return None;
        }
        self.emitted.retain(|(u, _)| !abandoned.contains(u));
        self.emitted_set.retain(|u| !abandoned.contains(u));
        let resend = next_prompt.is_some_and(|p| {
            off.iter().any(|k| matches!(k, Emitted::Prompt(_)))
                && off.iter().all(|k| match k {
                    Emitted::Prompt(t) => t.trim() == p.trim(),
                    Emitted::Interruption => true,
                    Emitted::Other => false,
                })
        });
        (!resend).then(|| self.emitted.last().map(|(u, _)| u.clone()))
    }
}

/// uuids of the entries of `main` on an abandoned Branch, with the Main Line going through `leaf`
/// (a prompt that forked, at byte `upto`; later entries are ignored; Claude Code's own leaf
/// marker is written late) down to its newest descendant (children written before their parent
/// stay on it).
/// Entries in the Main Line's tree or under a real root (no parent) but not on the Main Line are
/// abandoned; entries cut off by a dangling parent are a fragment, not a Branch, and are kept.
/// The Main Line itself follows the transcript crate's rules (merged fragments, absorbed tool results, compact
/// boundaries). `None` if unreadable.
pub fn off_main_uuids(main: &Path, leaf: &str, upto: u64) -> Option<HashSet<String>> {
    let mut chunk = transcript::parse::read_entries(main, 0).ok()?;
    chunk.records.retain(|r| r.offset <= upto);
    let real_root: HashSet<String> = chunk
        .records
        .iter()
        .filter(|r| r.entry.parent_uuid.is_none() && r.entry.logical_parent_uuid.is_none())
        .filter_map(|r| r.entry.uuid.clone())
        .collect();
    let files = [LoadedFile {
        path: main.to_path_buf(),
        role: FileRole::Main,
        agent_id: None,
        records: chunk.records,
    }];
    let mut s = skeleton::build(&files);
    // Root of every node, memoized (parents are cycle-free).
    let mut root: Vec<Option<usize>> = vec![None; s.nodes.len()];
    let mut root_of = |i: usize| {
        let mut path = vec![];
        let mut v = i;
        let r = loop {
            if let Some(r) = root[v] {
                break r;
            }
            match s.nodes[v].parent {
                Some(p) => {
                    path.push(v);
                    v = p;
                }
                None => break v,
            }
        };
        root[v] = Some(r);
        for u in path {
            root[u] = Some(r);
        }
        r
    };
    let leaf = tree::descend(&s, *s.by_uuid.get(leaf)?);
    s.main_path = tree::root_path(&s, leaf);
    let on_main = tree::main_set(&s);
    let main_root = root_of(*s.main_path.first()?);
    Some(
        (0..s.nodes.len())
            .filter(|&i| {
                let r = root_of(i);
                (r == main_root || real_root.contains(&s.nodes[r].uuid))
                    && !on_main.contains(&s.nodes[i].uuid)
            })
            .map(|i| s.nodes[i].uuid.clone())
            .collect(),
    )
}

/// The prompt text of a main-file user prompt entry.
fn prompt_text(l: &Line) -> Option<String> {
    let e = &l.entry;
    if e.entry_type() != "user" || e.is_meta == Some(true) {
        return None;
    }
    match e.message.as_ref()?.content.as_ref()? {
        RawContent::Text(t) if !prompt::is_command_record(t) && !prompt::is_interruption(t) => {
            Some(prompt::unwrap_pasted(t))
        }
        _ => None,
    }
}
