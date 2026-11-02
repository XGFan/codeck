//! The analyzer: asks a model (an Anthropic-compatible Messages API, DeepSeek by default) what an
//! unrecognized dialog is and returns it as a [`Layout`] (ADR-0002). It sends the whole screen and
//! the pending tool calls. Configured by environment variables; without a key it is off.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::layout::{Layout, LayoutKind, LayoutOption, Meaning};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/anthropic";
pub const DEFAULT_MODEL: &str = "deepseek-flash";

#[derive(Clone)]
pub struct Analyzer {
    pub base_url: String,
    pub model: String,
    key: String,
}

impl std::fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyzer")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("key", &"***")
            .finish()
    }
}

/// Plain http only to this machine; the key never goes out in clear.
fn is_loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or_default();
    // `user:pass@host`: the host is after the last `@`.
    let authority = authority.rsplit('@').next().unwrap_or_default();
    let host = authority.split(':').next().unwrap_or_default();
    matches!(host, "localhost" | "127.0.0.1")
}

/// Largest response read from the model service.
const MAX_RESPONSE: u64 = 1 << 20;

/// What the analyzer is shown.
pub struct Request<'a> {
    pub lines: &'a [String],
    /// The Process File's `waitingFor`.
    pub waiting_for: &'a str,
    /// Pending tool calls `(name, input)`.
    pub pending: Vec<(String, Value)>,
    pub claude_version: Option<String>,
}

impl Analyzer {
    /// `CCRPC_ANALYZER_API_KEY` (required), `CCRPC_ANALYZER_BASE_URL`, `CCRPC_ANALYZER_MODEL`.
    pub fn from_env() -> Option<Analyzer> {
        let key = std::env::var("CCRPC_ANALYZER_API_KEY").ok()?;
        if key.trim().is_empty() {
            return None;
        }
        Some(Analyzer {
            base_url: std::env::var("CCRPC_ANALYZER_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_BASE_URL.into()),
            model: std::env::var("CCRPC_ANALYZER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
            key,
        })
    }

    /// Asks the model; the layout still has to pass its structural checks on the screen.
    pub fn analyze(&self, req: &Request) -> Result<Layout> {
        let body = json!({
            "model": self.model,
            "max_tokens": 2000,
            "system": SYSTEM,
            "messages": [{"role": "user", "content": prompt(req)}],
        });
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        // No redirects (they would carry the key elsewhere), https except to this machine.
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .https_only(!is_loopback(&self.base_url))
            .timeout(Duration::from_secs(60))
            .build();
        let resp = agent
            .post(&url)
            .set("x-api-key", &self.key)
            .set("authorization", &format!("Bearer {}", self.key))
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json")
            .send_json(body)
            .map_err(|e| anyhow!("分析器请求失败：{e}"))?;
        let mut buf = Vec::new();
        {
            use std::io::Read;
            resp.into_reader()
                .take(MAX_RESPONSE)
                .read_to_end(&mut buf)
                .context("读不完分析器的响应")?;
        }
        let v: Value = serde_json::from_slice(&buf).context("分析器的响应不是 JSON")?;
        let text: String = v["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str())
            .collect();
        let mut layout = parse_answer(&text)?;
        layout.claude_version = req.claude_version.clone();
        layout.screen = req.lines.to_vec();
        layout.id = layout.compute_id();
        Ok(layout)
    }
}

const SYSTEM: &str = "You read the text of a terminal running Claude Code (an interactive CLI) \
that is stopped on a dialog waiting for the user, and describe the dialog so a program can \
answer it by pressing the option's number key. Answer with one JSON object and nothing else.";

fn prompt(req: &Request) -> String {
    let screen: String = req
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{i:02}|{l}\n"))
        .collect();
    let pending: Vec<Value> = req
        .pending
        .iter()
        .map(|(n, i)| {
            let mut s = i.to_string();
            if s.len() > 2000 {
                s.truncate(s.floor_char_boundary(2000));
            }
            json!({"tool": n, "input": s})
        })
        .collect();
    format!(
        r#"Claude Code reports it is waiting for: {waiting:?}
Pending tool calls (not yet answered): {pending}

Screen (row|text):
{screen}
Describe the dialog at the bottom of the screen as JSON:
{{
  "kind": "permission" | "choice" | "none",
  "title": "<the dialog's question line, copied exactly, without leading/trailing spaces>",
  "options": [
    {{"number": 1, "label": "<the start of the option's text, copied exactly; leave out parts that change from one call to the next such as commands, paths or file names>", "meaning": "allow" | "allow_always" | "deny" | "other", "description": "<what choosing it does, in a few words>"}}
  ]
}}
- "permission": the dialog asks whether to let a pending tool call run. Then "allow" lets it run once, "allow_always" lets it run and stops asking for similar calls, "deny" refuses. It must have at least one "allow" and one "deny" option.
- "choice": any other numbered menu waiting for the user (approving a plan, confirming an exit, a startup notice). Use "allow" for options that go ahead, "deny" for options that refuse or cancel, "other" for the rest.
- "none": there is no numbered menu waiting for an answer.
- List every numbered option of that menu, in order, with its number as shown."#,
        waiting = req.waiting_for,
        pending = Value::Array(pending),
    )
}

/// The model's JSON (possibly wrapped in prose or a code fence) as a layout.
pub fn parse_answer(text: &str) -> Result<Layout> {
    let start = text.find('{').ok_or_else(|| anyhow!("分析器没有给出 JSON"))?;
    let end = text.rfind('}').ok_or_else(|| anyhow!("分析器没有给出 JSON"))?;
    let v: Value = serde_json::from_str(&text[start..=end]).context("分析器给出的 JSON 读不出")?;
    let kind = match v["kind"].as_str() {
        Some("permission") => LayoutKind::Permission,
        Some("choice") => LayoutKind::Choice,
        Some("none") => bail!("分析器认为屏幕上没有等待作答的菜单"),
        other => bail!("分析器给出的 kind 不对：{other:?}"),
    };
    let title = v["title"].as_str().unwrap_or_default().trim().to_string();
    let options = v["options"]
        .as_array()
        .ok_or_else(|| anyhow!("分析器没有给出 options"))?
        .iter()
        .map(|o| {
            let meaning = match o["meaning"].as_str() {
                Some("allow") => Meaning::Allow,
                Some("allow_always") => Meaning::AllowAlways,
                Some("deny") => Meaning::Deny,
                _ => Meaning::Other,
            };
            Ok(LayoutOption {
                number: o["number"]
                    .as_u64()
                    .ok_or_else(|| anyhow!("分析器给出的选项没有 number"))?
                    as u32,
                label: o["label"].as_str().unwrap_or_default().trim().to_string(),
                meaning,
                description: o["description"].as_str().map(str::to_string),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Layout {
        id: String::new(),
        kind,
        title,
        options,
        source: String::new(),
        claude_version: None,
        learned_at: Some(now_rfc3339()),
        screen: Vec::new(),
    })
}

fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil date from days since the epoch (UTC).
    let days = (secs / 86_400) as i64;
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_fenced_answer() {
        let text = "Here it is:\n```json\n{\"kind\":\"choice\",\"title\":\"Would you like to proceed?\",\"options\":[{\"number\":1,\"label\":\"Yes\",\"meaning\":\"allow\"},{\"number\":2,\"label\":\"No\",\"meaning\":\"deny\",\"description\":\"cancel\"}]}\n```";
        let l = parse_answer(text).unwrap();
        assert_eq!(l.kind, LayoutKind::Choice);
        assert_eq!(l.title, "Would you like to proceed?");
        assert_eq!(l.options[1].meaning, Meaning::Deny);
        assert!(l.learned_at.as_deref().unwrap().ends_with('Z'));
    }

    #[test]
    fn key_stays_out_of_debug_and_http_only_to_loopback() {
        let a = Analyzer {
            base_url: "https://x".into(),
            model: "m".into(),
            key: "sk-secret".into(),
        };
        assert!(!format!("{a:?}").contains("sk-secret"));
        assert!(is_loopback("http://127.0.0.1:8080/v1"));
        assert!(is_loopback("http://localhost/anthropic"));
        assert!(!is_loopback("http://api.example.com/anthropic"));
        assert!(!is_loopback("http://localhost:x@evil.example/anthropic"));
    }

    #[test]
    fn refuses_no_menu_and_bad_kinds() {
        assert!(parse_answer("{\"kind\":\"none\",\"options\":[]}").is_err());
        assert!(parse_answer("{\"kind\":\"menu\",\"options\":[]}").is_err());
        assert!(parse_answer("no json here").is_err());
    }
}
