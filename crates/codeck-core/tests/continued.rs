//! A Session moved to the background (`continued-in`): the Session carrying it on copies its whole
//! conversation, so only that one is listed, counted, searched and billed; the old one opens as it.
//! The copy leaves the old Session's Subagent Runs and `tool-results/` behind: the new one takes
//! them from the old one's directory.
mod common;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use codeck_core::model::{
    DetailSource, SearchRequest, SessionQuery, StatsRequest, ToolDetailRequest, ToolPart,
    TranscriptRequest, TranscriptScope,
};
use common::engine::{Env, data_root};
use common::ids::{CONTINUED_DIR, CONTINUED_NEW as NEW, CONTINUED_OLD as OLD};

/// The Subagent Run of the copied Agent call; its files are only in the old Session's directory.
const AGENT: &str = "a5e1c0d7f2b9a4e36";
const AGENT_CALL: &str = "toolu_01CZ4fy78CDdkmtVqGSSatxR";
/// A copied tool call whose output was saved to the old Session's `tool-results/`.
const PERSISTED_CALL: &str = "toolu_01KjgPwRANne8voYmPHMpAuf";

fn transcript(env: &Env, sid: &str, scope: TranscriptScope) -> codeck_core::model::Transcript {
    env.engine
        .get_transcript(&TranscriptRequest {
            session_id: sid.into(),
            scope,
            branch_choices: Default::default(),
            include_hidden: false,
        })
        .unwrap()
}

fn file(env: &Env, sid: &str) -> PathBuf {
    env.root
        .path()
        .join("projects")
        .join(CONTINUED_DIR)
        .join(format!("{sid}.jsonl"))
}

fn listed(env: &Env) -> Vec<String> {
    let q = SessionQuery::default();
    let mut ids: Vec<String> = env
        .engine
        .list_sessions(&q)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    ids.sort();
    ids
}

fn search_groups(env: &Env, query: &str) -> Vec<String> {
    let r = env
        .engine
        .search(&SearchRequest {
            query: query.to_owned(),
            project_id: None,
            max_sessions: 50,
            hits_per_session: 50,
        })
        .unwrap();
    r.groups.into_iter().map(|g| g.session.id).collect()
}

/// (output tokens, cost) of a session.
fn billed(env: &Env, sid: &str) -> (f64, f64) {
    let s = env.engine.get_session(sid).unwrap().summary;
    (s.tokens.output, s.cost)
}

#[test]
fn only_the_continued_session_is_listed_and_the_old_one_opens_as_it() {
    let env = Env::indexed(data_root(&["continued"]));
    assert_eq!(listed(&env), vec![NEW]);
    let projects = env.engine.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].session_count, 1);
    assert_eq!(env.engine.index_status().sessions_total, 1);

    let old = env.engine.get_session(OLD).unwrap();
    assert_eq!(old.superseded_by.as_deref(), Some(NEW));
    let new = env.engine.get_session(NEW).unwrap();
    assert_eq!(new.superseded_by, None);
    // The same conversation carried on, not a Fork: no Origin Session, no Fork child, nothing folded.
    assert!(new.summary.fork_origin.is_none());
    assert!(old.forks.is_empty() && new.forks.is_empty());
    let t = transcript(&env, NEW, TranscriptScope::Main);
    assert!(t.inherited.is_none());
    assert!(t.nodes.iter().all(|n| !n.inherited));

    // The copied Agent call links the old Session's Subagent Run, which counts under the new one.
    let run = t.subagents.iter().find(|r| r.agent_id == AGENT).unwrap();
    assert_eq!(run.tool_use_id.as_deref(), Some(AGENT_CALL));
    assert_eq!(run.message_count, 2);
    let sub = transcript(
        &env,
        NEW,
        TranscriptScope::Subagent {
            agent_id: AGENT.into(),
        },
    );
    assert!(!sub.nodes.is_empty());
    assert_eq!(new.summary.subagent_count, 1);
    assert_eq!(new.tokens_subagents.output, 120.0);
    let path = env.engine.session_file_path(NEW, Some(AGENT)).unwrap();
    assert!(path.to_string_lossy().contains(OLD), "{path:?}");
    // A copied tool call reads its saved output from the old Session's `tool-results/`.
    let out = env
        .engine
        .get_tool_detail(&ToolDetailRequest {
            session_id: NEW.into(),
            scope: TranscriptScope::Main,
            tool_use_id: PERSISTED_CALL.into(),
            part: ToolPart::Output,
        })
        .unwrap();
    assert_eq!(out.source, DetailSource::PersistedFile);

    // A term of the copied conversation hits the continued session only.
    assert_eq!(search_groups(&env, "lorem"), vec![NEW]);
}

#[test]
fn the_continued_session_bills_the_copy_and_totals_do_not_change() {
    // Billed alone, with the old one's Subagent Runs in its own directory, the continued session
    // holds every message of the conversation.
    let alone = {
        let root = data_root(&["continued"]);
        let dir = root.path().join("projects").join(CONTINUED_DIR);
        std::fs::remove_file(dir.join(format!("{OLD}.jsonl"))).unwrap();
        std::fs::rename(dir.join(OLD), dir.join(NEW)).unwrap();
        let env = Env::scanned(root);
        assert_eq!(
            env.engine.get_session(NEW).unwrap().summary.subagent_count,
            1
        );
        billed(&env, NEW)
    };
    assert!(alone.0 > 0.0 && alone.1 > 0.0);

    let env = Env::scanned(data_root(&["continued"]));
    assert_eq!(billed(&env, NEW), alone);
    assert_eq!(billed(&env, OLD), (0.0, 0.0));
    let stats = env.engine.stats(&StatsRequest::default()).unwrap();
    assert_eq!(stats.overview.output_tokens, alone.0);
    assert!((stats.overview.cost - alone.1).abs() < 1e-9);
    assert_eq!(stats.overview.sessions, 1);
    let ranked: Vec<&str> = stats
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(ranked, vec![NEW]);
}

#[test]
fn without_a_full_copy_in_the_index_both_sessions_are_shown() {
    let env = Env::indexed(data_root(&["continued"]));
    let old_alone = {
        let root = data_root(&["continued"]);
        std::fs::remove_file(
            root.path()
                .join("projects")
                .join(CONTINUED_DIR)
                .join(format!("{NEW}.jsonl")),
        )
        .unwrap();
        billed(&Env::scanned(root), OLD)
    };

    // The continued session is gone: the old one is listed and billed again.
    let new_file = file(&env, NEW);
    let saved = std::fs::read(&new_file).unwrap();
    std::fs::remove_file(&new_file).unwrap();
    let cs = env
        .engine
        .apply_changes(std::slice::from_ref(&new_file))
        .unwrap();
    assert!(
        cs.changed.contains(&OLD.to_owned()) && cs.projects_changed,
        "{cs:?}"
    );
    assert_eq!(listed(&env), vec![OLD]);
    let old = env.engine.get_session(OLD).unwrap();
    assert_eq!(old.superseded_by, None);
    // It has its Subagent Run back.
    assert_eq!(old.summary.subagent_count, 1);
    assert_eq!(billed(&env, OLD), old_alone);

    // Back again: the old one is superseded once more.
    std::fs::write(&new_file, &saved).unwrap();
    let cs = env.engine.apply_changes(&[new_file]).unwrap();
    assert!(cs.changed.contains(&OLD.to_owned()), "{cs:?}");
    assert_eq!(listed(&env), vec![NEW]);
    assert_eq!(billed(&env, OLD), (0.0, 0.0));
    assert_eq!(
        env.engine.get_session(NEW).unwrap().summary.subagent_count,
        1
    );

    let old_file = file(&env, OLD);
    let append = |line: String| {
        let mut f = OpenOptions::new().append(true).open(&old_file).unwrap();
        writeln!(f, "{line}").unwrap();
        drop(f);
        env.engine
            .apply_changes(std::slice::from_ref(&old_file))
            .unwrap();
    };
    // A reply the copy left out (real copies drop replies that are only an empty thinking block)
    // does not matter: it stays billed to the old one, the continued one is still the only one.
    append(format!(
        r#"{{"type":"assistant","uuid":"5f3a9d27-0c81-4b6e-a2d4-8e17c3b90f52","parentUuid":null,"sessionId":"{OLD}","timestamp":"2026-11-06T01:30:00.000Z","message":{{"id":"msg_dropped","role":"assistant","model":"claude-opus-5-5","content":[{{"type":"thinking","thinking":""}}],"usage":{{"input_tokens":1,"output_tokens":3}}}}}}"#
    ));
    assert_eq!(listed(&env), vec![NEW]);
    assert_eq!(billed(&env, OLD).0, 3.0);

    // The old one goes on with a prompt the continued one lacks: both are shown.
    append(format!(
        r#"{{"type":"user","uuid":"0b9e2c51-7a3d-4f80-9c16-5e2d8a4b7f01","parentUuid":null,"sessionId":"{OLD}","timestamp":"2026-11-06T02:00:00.000Z","message":{{"role":"user","content":"one more question"}}}}"#
    ));
    let mut both = vec![NEW.to_owned(), OLD.to_owned()];
    both.sort();
    assert_eq!(listed(&env), both);
    assert_eq!(env.engine.get_session(OLD).unwrap().superseded_by, None);
}
