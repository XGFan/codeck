//! `dispatch` with JSON as a client sends it, against a copy of a codeck-core fixture.

#[path = "../../codeck-core/tests/common/fixtures.rs"]
mod fixtures;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use codeck_host::{Host, HostConfig, dispatch};
use serde_json::{Value, json};

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatches_json_calls_and_refuses_local_methods_remotely() {
    let root = tempfile::tempdir().unwrap();
    copy_dir(&fixtures::fixture_dir("basic"), root.path());
    let state = tempfile::tempdir().unwrap();
    let host = Host::start(
        HostConfig {
            cache_dir: state.path().join("cache"),
            settings_path: state.path().join("settings.json"),
            ccrpc_logs: Some(state.path().join("logs")),
            images: Some(state.path().join("images")),
            data_root: Some(root.path().to_path_buf()),
            ..HostConfig::desktop()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let call = |method: &'static str, args: Value, remote_only: bool| {
        let host = host.clone();
        async move { dispatch(&host, method, args, remote_only).await }
    };

    // The first full scan runs in the background.
    let query = json!({ "query": { "projectIds": [], "sort": "lastActive", "descending": true } });
    let deadline = Instant::now() + Duration::from_secs(20);
    let sessions = loop {
        let v = call("list_sessions", query.clone(), true).await.unwrap();
        if !v.as_array().unwrap().is_empty() || Instant::now() > deadline {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let id = fixtures::ids::BASIC;
    assert!(
        sessions.as_array().unwrap().iter().any(|s| s["id"] == id),
        "{sessions}"
    );

    let projects = call("list_projects", Value::Null, true).await.unwrap();
    assert!(!projects.as_array().unwrap().is_empty());

    let detail = call("get_session", json!({ "sessionId": id }), true)
        .await
        .unwrap();
    assert_eq!(detail["summary"]["id"], id, "{detail}");

    let req = json!({ "req": { "sessionId": id, "scope": { "kind": "main" }, "branchChoices": [], "includeHidden": false } });
    let transcript = call("get_transcript", req, true).await.unwrap();
    assert_eq!(transcript["sessionId"], id);
    assert!(!transcript["nodes"].as_array().unwrap().is_empty());

    let e = call("get_session", json!({ "session_id": id }), true)
        .await
        .unwrap_err();
    assert!(e.message.starts_with("参数不对"), "{e}");

    let e = call("no_such_method", json!({}), false).await.unwrap_err();
    assert_eq!(serde_json::to_value(&e).unwrap()["code"], "notFound");

    // `local` methods: the desktop app only.
    let e = call("get_diagnostics", json!({}), true).await.unwrap_err();
    assert_eq!(serde_json::to_value(&e).unwrap()["code"], "invalidQuery");
    let e = call("set_data_root", json!({ "path": null }), true)
        .await
        .unwrap_err();
    assert!(e.message.contains("set_data_root"), "{e}");
    call("get_diagnostics", json!({}), false).await.unwrap();
}
