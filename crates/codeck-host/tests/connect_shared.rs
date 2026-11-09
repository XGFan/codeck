//! `connect_shared` with a fake ccrpc that, like the real one, refuses a second attach to a Holder.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use codeck_host::{Host, HostConfig, dispatch};
use serde_json::{Value, json};

const FAKE: &str = r#"#!/bin/sh
D="$(dirname "$0")"
case "$1" in
ls) exec sleep 60 ;;
attach)
  mkdir "$D/attached" 2>/dev/null || { echo "已有 ccrpc 连着" >&2; exit 1; }
  trap 'rmdir "$D/attached"' EXIT
  echo '{"type":"ready","protocol":2,"holder_id":"H1","session_id":"S1","cwd":"/w","state":"idle","size":[80,24]}'
  # The first claude in the Pane ends; its ccrpc stays until its stdin closes.
  if mkdir "$D/first" 2>/dev/null; then echo '{"type":"session_exit"}'; fi
  cat >/dev/null
  ;;
esac
"#;

async fn states(host: &Arc<Host>) -> Vec<Value> {
    dispatch(host, "connect_states", Value::Null, true)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// claude was started again in the Pane: Connect ends the old ccrpc, then attaches a new one.
#[tokio::test(flavor = "multi_thread")]
async fn connect_replaces_a_connection_whose_session_ended() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("ccrpc");
    std::fs::write(&fake, FAKE).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary, before anything else reads the environment.
    unsafe { std::env::set_var("CODECK_CCRPC", &fake) };
    let host = Host::start(
        HostConfig {
            cache_dir: dir.path().join("cache"),
            settings_path: dir.path().join("settings.json"),
            ccrpc_logs: Some(dir.path().join("logs")),
            images: Some(dir.path().join("images")),
            data_root: Some(dir.path().join("root")),
            refresh_prices: false,
            track_connect: true,
            ..HostConfig::desktop()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let connect = async || {
        dispatch(&host, "connect_shared", json!({ "holderId": "H1" }), true)
            .await
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    };

    let first = connect().await;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !states(&host)
        .await
        .iter()
        .any(|s| s["conn"] == first && s["sessionExited"] == true)
    {
        assert!(Instant::now() < deadline, "the first Session never ended");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let second = connect().await;
    assert_ne!(second, first, "an ended connection was handed back");
    // Refused (exit 1, removed) if the old ccrpc were still attached.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let all = states(&host).await;
        if all.len() == 1 && all[0]["conn"] == second && all[0]["ready"] == true {
            assert_eq!(all[0]["sessionExited"], false);
            break;
        }
        assert!(Instant::now() < deadline, "{all:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(connect().await, second, "a live connection is shared");
    host.shutdown();
}
