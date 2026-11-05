//! The analyzer: asks a model (an Anthropic-compatible Messages API, DeepSeek by default) what an
//! unrecognized dialog is and returns it as a [`Layout`] (ADR-0002). It sends the whole screen and
//! the pending tool calls. Configured by environment variables; without a key it is off.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::layout::{Layout, LayoutKind, LayoutOption, Meaning, Select};

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

/// The built-in dialog `ccrpc analyzer test` shows the analyzer (a short screen: a cheap request).
const TEST_DIALOG: &str = "Change effort level?";

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
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .max_redirects(0)
            // ureq 3 would otherwise pick up `https_proxy` / `all_proxy` from the environment; the
            // key goes straight to the analyzer as before.
            .proxy(None)
            .https_only(!is_loopback(&self.base_url))
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        let mut resp = agent
            .post(&url)
            .header("x-api-key", &self.key)
            .header("authorization", &format!("Bearer {}", self.key))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .send(serde_json::to_vec(&body)?)
            .map_err(|e| anyhow!("分析器请求失败：{e}"))?;
        // 4xx / 5xx are already errors; a redirect is returned as is (max_redirects 0) and refused here.
        if !resp.status().is_success() {
            bail!("分析器请求失败：HTTP {}", resp.status());
        }
        // A body over MAX_RESPONSE is an error rather than a truncated read.
        let buf = resp
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE)
            .read_to_vec()
            .context("读不完分析器的响应")?;
        let v: Value = serde_json::from_slice(&buf).context("分析器的响应不是 JSON")?;
        let text: String = v["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str())
            .collect();
        let mut layout = parse_answer(&text)?;
        // A tool approval needs a pending tool call: with none, the menu is a choice (a slash
        // command's confirmation, such as /add-dir's, reads like a tool approval).
        if req.pending.is_empty() && layout.kind == LayoutKind::Permission {
            layout.kind = LayoutKind::Choice;
        }
        layout.claude_version = req.claude_version.clone();
        layout.screen = req.lines.to_vec();
        layout.id = layout.compute_id();
        Ok(layout)
    }

    /// `ccrpc analyzer test`: asks about a built-in dialog and accepts the answer only as a learned
    /// layout would be (its structural checks on that screen), and of the built-in layout's kind.
    /// Returns the layout and how long the request took; nothing goes into the layout library.
    pub fn test(&self) -> Result<(Layout, Duration)> {
        let sample = crate::layout::builtin()
            .into_iter()
            .find(|l| l.title == TEST_DIALOG)
            .ok_or_else(|| anyhow!("内置布局里没有测试用的对话框"))?;
        let started = Instant::now();
        let got = self.analyze(&Request {
            lines: &sample.screen,
            waiting_for: "input needed",
            pending: Vec::new(),
            claude_version: sample.claude_version.clone(),
        })?;
        let elapsed = started.elapsed();
        if got.kind != sample.kind || got.match_screen(&sample.screen).is_none() {
            bail!("分析结果与屏幕对不上");
        }
        Ok((got, elapsed))
    }
}

const SYSTEM: &str = "You read the text of a terminal running Claude Code (an interactive CLI) \
that is stopped on a dialog waiting for the user, and describe the dialog so a program can \
answer it: by pressing the option's number key, or, when the options have no numbers, by moving \
the ❯ cursor onto the option and pressing Enter. Answer with one JSON object and nothing else.";

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
  "title": "<the dialog's question line, copied exactly, without leading/trailing spaces; if the screen wraps it over several rows, join them with a space>",
  "select": "number" | "cursor",
  "options": [
    {{"number": 1, "label": "<the start of the option's text, copied exactly; leave out parts that change from one call to the next such as commands, paths or file names>", "meaning": "allow" | "allow_always" | "deny" | "other", "description": "<what choosing it does, in a few words>"}}
  ]
}}
- "permission": the dialog asks whether to let one of the pending tool calls above run (never when there are none). Then "allow" lets it run once, "allow_always" lets it run and stops asking for similar calls, "deny" refuses. It must have at least one "allow" and one "deny" option.
- "choice": any other numbered menu waiting for the user (approving a plan, confirming an exit, a startup notice, confirming a slash command). Use "allow" for options that go ahead, "deny" for options that refuse or cancel, "other" for the rest.
- "none": there is no menu waiting for an answer.
- "select": "number" when every option is shown with its number ("❯ 1. Yes"); "cursor" when the options have no numbers and the one marked ❯ is picked with the arrow keys and Enter. Then "number" is the option's position, from 1.
- List every option of that menu, in order, with its number as shown (its position for "cursor")."#,
        waiting = req.waiting_for,
        pending = Value::Array(pending),
    )
}

/// The model's JSON (possibly wrapped in prose or a code fence) as a layout.
pub fn parse_answer(text: &str) -> Result<Layout> {
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("分析器没有给出 JSON"))?;
    let end = text
        .rfind('}')
        .ok_or_else(|| anyhow!("分析器没有给出 JSON"))?;
    let v: Value = serde_json::from_str(&text[start..=end]).context("分析器给出的 JSON 读不出")?;
    let kind = match v["kind"].as_str() {
        Some("permission") => LayoutKind::Permission,
        Some("choice") => LayoutKind::Choice,
        Some("none") => bail!("分析器认为屏幕上没有等待作答的菜单"),
        other => bail!("分析器给出的 kind 不对：{other:?}"),
    };
    let title = v["title"].as_str().unwrap_or_default().trim().to_string();
    let select = match v["select"].as_str() {
        Some("cursor") => Select::Cursor,
        _ => Select::Number,
    };
    let options = v["options"]
        .as_array()
        .ok_or_else(|| anyhow!("分析器没有给出 options"))?
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let meaning = match o["meaning"].as_str() {
                Some("allow") => Meaning::Allow,
                Some("allow_always") => Meaning::AllowAlways,
                Some("deny") => Meaning::Deny,
                _ => Meaning::Other,
            };
            let number = match select {
                // A cursor menu shows no numbers: an option is its position.
                Select::Cursor => i as u32 + 1,
                Select::Number => o["number"]
                    .as_u64()
                    .ok_or_else(|| anyhow!("分析器给出的选项没有 number"))?
                    as u32,
            };
            Ok(LayoutOption {
                number,
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
        select,
        tool: None,
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
    fn a_cursor_menu_answer_numbers_options_by_position() {
        let text = r#"{"kind":"permission","title":"Computer Use wants to control these apps","select":"cursor","options":[{"label":"Deny, and tell Claude what to do","meaning":"deny"},{"number":7,"label":"Allow for this session","meaning":"allow"}]}"#;
        let l = parse_answer(text).unwrap();
        assert_eq!(l.select, Select::Cursor);
        assert_eq!(
            l.options.iter().map(|o| o.number).collect::<Vec<_>>(),
            [1, 2]
        );
        // Without `select` it is a numbered menu, and numbers are required.
        assert!(parse_answer(&text.replace(r#""select":"cursor","#, "")).is_err());
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

    /// A model service on this machine answering each request with the next of `texts` as the
    /// reply's text.
    fn serve_answers(texts: Vec<String>) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for text in texts {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                reader.read_exact(&mut vec![0; len]).unwrap();
                let body = json!({"content": [{"type": "text", "text": text}]}).to_string();
                let mut stream = stream;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (base_url, server)
    }

    #[test]
    fn test_accepts_only_an_answer_that_fits_the_sample_dialog() {
        let right = json!({"kind": "choice", "title": TEST_DIALOG, "options": [
            {"number": 1, "label": "Yes, switch to", "meaning": "allow"},
            {"number": 2, "label": "No, go back", "meaning": "deny"}]});
        // Labels not on the screen: the structural checks refuse it, as they would a learned layout.
        let wrong = json!({"kind": "choice", "title": TEST_DIALOG, "options": [
            {"number": 1, "label": "Yes", "meaning": "allow"},
            {"number": 2, "label": "Cancel", "meaning": "deny"}]});
        let (base_url, server) = serve_answers(vec![right.to_string(), wrong.to_string()]);
        let a = Analyzer {
            base_url,
            model: "m".into(),
            key: "k".into(),
        };
        let (layout, _) = a.test().unwrap();
        assert_eq!(layout.kind, LayoutKind::Choice);
        assert_eq!(layout.options.len(), 2);
        let err = a.test().unwrap_err().to_string();
        assert_eq!(err, "分析结果与屏幕对不上");
        server.join().unwrap();
    }

    /// /add-dir's confirmation reads like a tool approval, and the analyzer took it for one (seen
    /// live): with no pending tool call it is a choice, else it could never be announced.
    #[test]
    fn a_permission_answer_without_a_pending_tool_is_a_choice() {
        let screen = crate::layout::builtin()
            .into_iter()
            .find(|l| l.title == "Add directory to workspace")
            .unwrap()
            .screen;
        let answer =
            json!({"kind": "permission", "title": "Add directory to workspace", "options": [
            {"number": 1, "label": "Yes, for this session", "meaning": "allow"},
            {"number": 2, "label": "Yes, and remember this directory", "meaning": "allow_always"},
            {"number": 3, "label": "No", "meaning": "deny"}]})
            .to_string();
        let (base_url, server) = serve_answers(vec![answer.clone(), answer]);
        let a = Analyzer {
            base_url,
            model: "m".into(),
            key: "k".into(),
        };
        let ask = |pending: Vec<(String, Value)>| {
            a.analyze(&Request {
                lines: &screen,
                waiting_for: "dialog open",
                pending,
                claude_version: None,
            })
            .unwrap()
        };
        let l = ask(Vec::new());
        assert_eq!(l.kind, LayoutKind::Choice);
        assert!(l.match_screen(&screen).is_some());
        let bash = vec![("Bash".to_string(), json!({"command": "ls"}))];
        assert_eq!(ask(bash).kind, LayoutKind::Permission);
        server.join().unwrap();
    }

    #[test]
    fn refuses_a_redirect_and_sends_the_key_headers() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                head.push(line.trim_end().to_ascii_lowercase());
            }
            let mut stream = stream;
            stream
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            head
        });
        let a = Analyzer {
            base_url,
            model: "m".into(),
            key: "k-123".into(),
        };
        let lines = vec!["Do you want to proceed?".to_string()];
        let req = Request {
            lines: &lines,
            waiting_for: "approve",
            pending: Vec::new(),
            claude_version: None,
        };
        let err = a.analyze(&req).unwrap_err().to_string();
        assert!(err.starts_with("分析器请求失败"), "{err}");
        let head = server.join().unwrap();
        assert_eq!(head[0], "post /v1/messages http/1.1");
        assert!(head.contains(&"x-api-key: k-123".to_string()), "{head:?}");
        assert!(
            head.contains(&"anthropic-version: 2023-06-01".to_string()),
            "{head:?}"
        );
    }
}
