use codeck_core::Engine;
use codeck_core::model::{
    JumpRequest, SearchHit, SearchRequest, SearchRole, SessionSummary, TranscriptRequest,
    TranscriptScope,
};
use serde::Serialize;

use crate::args::Parsed;
use crate::common::{self, CliError, Context};
use crate::turns;

const DEFAULT_LIMIT: u64 = 20;
const HITS_PER_SESSION: u32 = 5;
const SNIPPET_CHARS: usize = 300;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    indexed_at: Option<String>,
    /// False while the full-text backlog is still being indexed: results may be incomplete.
    index_complete: bool,
    total_sessions: u32,
    total_hits: u32,
    results: Vec<Group>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    session_id: String,
    title: String,
    project: Option<String>,
    status: &'static str,
    hit_count: u32,
    hits: Vec<Hit>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    /// The `show --turn` number; null when the hit is on no Main Line turn (see `location`).
    turn: Option<u32>,
    location: Location,
    /// With `location: subagent`: the id for `show --agent`.
    agent_id: Option<String>,
    role: &'static str,
    at: Option<String>,
    /// The match inside `**…**`.
    snippet: String,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
enum Location {
    MainLine,
    /// A Branch the conversation left (rewound or edited).
    Branch,
    Subagent,
    /// Not on the Main Line as it now reads (the file changed since it was indexed, usually).
    Unknown,
}

pub fn run(cx: &Context, a: &Parsed) -> Result<Output, CliError> {
    let query = a.one_positional("搜索词")?.to_owned();
    let limit = a.number("--limit", Some(DEFAULT_LIMIT))?.unwrap_or(0) as u32;
    let engine = cx.engine()?;
    let project_id = match a.value("--project") {
        Some(p) => Some(common::find_project(&engine, p)?.id),
        None => None,
    };
    let projects = common::project_paths(&engine)?;
    let resp = engine.search(&SearchRequest {
        query,
        project_id,
        max_sessions: limit,
        hits_per_session: HITS_PER_SESSION,
    })?;
    let results = resp
        .groups
        .into_iter()
        .map(|g| Group {
            status: common::status_word(g.session.live.as_ref()),
            project: projects.get(&g.session.project_id).cloned(),
            hit_count: g.hit_count,
            hits: locate(&engine, &g.session, &g.hits),
            title: g.session.title,
            session_id: g.session.id,
        })
        .collect();
    Ok(Output {
        indexed_at: engine.indexed_at().map(common::time_of),
        index_complete: resp.index_complete,
        total_sessions: resp.total_sessions,
        total_hits: resp.total_hits,
        results,
    })
}

/// Where each hit is. A hit on the Main Line of the Session as it now reads gets its turn number;
/// the rest say why not, rather than guess. One unreadable Session does not fail the search.
fn locate(engine: &Engine, s: &SessionSummary, hits: &[SearchHit]) -> Vec<Hit> {
    let transcript = engine
        .get_transcript(&TranscriptRequest {
            session_id: s.id.clone(),
            scope: TranscriptScope::Main,
            branch_choices: Vec::new(),
            include_hidden: true,
        })
        .ok();
    let numbers = transcript.as_ref().map(turns::numbers).unwrap_or_default();
    hits.iter()
        .map(|h| {
            let (location, turn) = if h.agent_id.is_some() {
                (Location::Subagent, None)
            } else {
                match engine.resolve_jump(&JumpRequest {
                    session_id: s.id.clone(),
                    node_id: h.node_id.clone(),
                    agent_id: None,
                    tool_use_id: h.tool_use_id.clone(),
                }) {
                    Ok(t) if t.in_abandoned_branch => (Location::Branch, None),
                    Ok(t) => match numbers.get(t.node_id.as_str()) {
                        Some(n) => (Location::MainLine, Some(*n)),
                        None => (Location::Unknown, None),
                    },
                    Err(_) => (Location::Unknown, None),
                }
            };
            Hit {
                turn,
                location,
                agent_id: h.agent_id.clone(),
                role: match h.role {
                    SearchRole::User => "user",
                    SearchRole::Assistant => "assistant",
                    SearchRole::ToolInput => "toolInput",
                },
                at: h.timestamp_ms.and_then(common::time_of_ms),
                snippet: common::cap(&common::one_line(&snippet(h)), SNIPPET_CHARS),
            }
        })
        .collect()
}

fn snippet(h: &SearchHit) -> String {
    h.snippet
        .iter()
        .map(|p| {
            if p.hit {
                format!("**{}**", p.text)
            } else {
                p.text.clone()
            }
        })
        .collect()
}
