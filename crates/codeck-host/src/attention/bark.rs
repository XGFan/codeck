//! Pushes to the iPhone through Bark (ADR-0019): a POST to the Bark server, which hands it to Apple's push service for
//! the Bark app. A notification's `id` takes the place of an earlier one with it; `delete` takes it away (Bark sends that as
//! a silent push, so it needs Background App Refresh on for Bark).

use std::time::Duration;

use serde_json::{Value, json};

pub const DEFAULT_SERVER: &str = "https://api.day.app";
/// What a CoDeck notification is grouped under in Notification Center and Bark's history.
const GROUP: &str = "CoDeck";
const TIMEOUT: Duration = Duration::from_secs(10);

/// One notification to push.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    /// Takes the place of an earlier one with it; `None`: none (the test push).
    pub id: Option<String>,
    pub title: String,
    pub body: String,
    /// Opened when it is tapped.
    pub url: Option<String>,
    pub time_sensitive: bool,
}

/// The request body for `push`. Not kept in Bark's history: each Session has one notification at most, and it goes
/// once settled.
pub fn push_body(device_key: &str, p: &Push) -> Value {
    let mut v = json!({
        "device_key": device_key,
        "title": p.title,
        "body": p.body,
        "group": GROUP,
        "isArchive": "0",
    });
    if let Some(id) = &p.id {
        v["id"] = json!(id);
    }
    if let Some(url) = &p.url {
        v["url"] = json!(url);
    }
    if p.time_sensitive {
        v["level"] = json!("timeSensitive");
    }
    v
}

/// The request body that takes the notification `id` away.
pub fn delete_body(device_key: &str, id: &str) -> Value {
    json!({ "device_key": device_key, "id": id, "delete": "1" })
}

/// The server as saved: trimmed, without a trailing `/`, the default when blank.
pub fn server_or_default(server: Option<&str>) -> String {
    match server.map(str::trim).map(|s| s.trim_end_matches('/')) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => DEFAULT_SERVER.to_string(),
    }
}

/// POSTs `body` to `<server>/push`; `Err` is why it failed, for people.
pub fn send(server: &str, body: &Value) -> Result<(), String> {
    let url = format!("{server}/push");
    let mut res = ureq::post(&url)
        .config()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .header("Content-Type", "application/json; charset=utf-8")
        .send(body.to_string())
        .map_err(|e| format!("连不上 Bark 服务器：{e}"))?;
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().unwrap_or_default();
    reply(status, &text)
}

/// What Bark's answer says: its JSON `code` is 200 when the push went out.
fn reply(status: u16, text: &str) -> Result<(), String> {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let code = v["code"].as_u64().unwrap_or(u64::from(status));
    if status == 200 && code == 200 {
        return Ok(());
    }
    match v["message"].as_str().filter(|m| !m.is_empty()) {
        Some(m) => Err(format!("Bark 返回 {code}：{m}")),
        None => Err(format!("Bark 返回 {code}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_carries_its_id_link_and_level() {
        let p = Push {
            id: Some("session-a".into()),
            title: "修登录".into(),
            body: "等待作答 3 分钟 · codeck".into(),
            url: Some("codeck://session/a".into()),
            time_sensitive: true,
        };
        assert_eq!(
            push_body("K", &p),
            json!({
                "device_key": "K",
                "title": "修登录",
                "body": "等待作答 3 分钟 · codeck",
                "group": "CoDeck",
                "isArchive": "0",
                "id": "session-a",
                "url": "codeck://session/a",
                "level": "timeSensitive",
            })
        );
        let plain = push_body(
            "K",
            &Push {
                id: None,
                url: None,
                time_sensitive: false,
                ..p
            },
        );
        assert!(
            plain.get("id").is_none() && plain.get("url").is_none() && plain.get("level").is_none()
        );
        assert_eq!(
            delete_body("K", "session-a"),
            json!({ "device_key": "K", "id": "session-a", "delete": "1" })
        );
    }

    #[test]
    fn the_server_defaults_and_loses_its_trailing_slash() {
        assert_eq!(server_or_default(None), DEFAULT_SERVER);
        assert_eq!(server_or_default(Some("  ")), DEFAULT_SERVER);
        assert_eq!(
            server_or_default(Some(" https://b.example/ ")),
            "https://b.example"
        );
    }

    #[test]
    fn barks_answer_says_whether_it_went_out() {
        assert_eq!(reply(200, r#"{"code":200,"message":"success"}"#), Ok(()));
        assert_eq!(
            reply(
                400,
                r#"{"code":400,"message":"failed to get device token"}"#
            ),
            Err("Bark 返回 400：failed to get device token".into())
        );
        assert_eq!(reply(502, "<html>"), Err("Bark 返回 502".into()));
    }
}
