//! The desktop app's window and LAN clients sharing one Host's Connect, with a fake ccrpc that, like the real one,
//! refuses a second attach to a Holder and takes a moment to let go. One test: `CODECK_CCRPC` and the settings path
//! are process-wide.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use codeck_host::lan::LanConfig;
use codeck_host::lan::server::event_channel;
use codeck_host::{Host, HostConfig, HostEvent};
use serde_json::{Value, json};

const FAKE: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$1" >> "$D/calls"
case "$1" in
ls)
  echo '[{"holder_id":"H1","plugin":true}]'
  exec cat >/dev/null
  ;;
attach)
  # attach --log <file> <holder>
  H="$4"
  mkdir "$D/attached-$H" 2>/dev/null || { echo "已有 ccrpc 连着" >&2; exit 1; }
  trap 'sleep 0.3; rmdir "$D/attached-$H"' EXIT
  echo "{\"type\":\"ready\",\"protocol\":2,\"holder_id\":\"$H\",\"session_id\":\"S\",\"cwd\":\"/w\",\"state\":\"idle\",\"size\":[80,24]}"
  cat >/dev/null
  ;;
esac
"#;

/// How many times ccrpc was run with this subcommand.
fn calls(dir: &Path, which: &str) -> usize {
    std::fs::read_to_string(dir.join("calls"))
        .unwrap_or_default()
        .lines()
        .filter(|l| *l == which)
        .count()
}

async fn eventually(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check().await {
        assert!(Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_and_lan_clients_share_connections_and_the_ls_watch() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("ccrpc");
    std::fs::write(&fake, FAKE).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary, before anything else reads the environment.
    unsafe { std::env::set_var("CODECK_CCRPC", &fake) };
    let seen: Arc<Mutex<Vec<HostEvent>>> = Arc::default();
    let log = seen.clone();
    let (lan_events, _) = event_channel();
    // As the desktop app is configured: Connect state tracked, no `ccrpc ls` of its own from the start.
    let host = Host::start(
        HostConfig {
            cache_dir: dir.path().join("cache"),
            settings_path: dir.path().join("settings.json"),
            ccrpc_logs: Some(dir.path().join("logs")),
            images: Some(dir.path().join("images")),
            data_root: Some(dir.path().join("root")),
            refresh_prices: false,
            track_connect: true,
            lan: Some(LanConfig {
                events: lan_events,
                web_dir: dir.path().join("web"),
                port: 1,
                subnets: Arc::new(|| Ok(Vec::new())),
            }),
            ..HostConfig::desktop()
        },
        Arc::new(move |e| log.lock().unwrap().push(e)),
    )
    .unwrap();
    let dir = dir.path();
    let events = || seen.lock().unwrap().clone();
    let shared = async |holder: &str| {
        host.remote_call("connect_shared", json!({ "holderId": holder }))
            .await
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    };
    let remote_conns = || {
        events()
            .into_iter()
            .filter_map(|e| match e {
                HostEvent::CcrpcRemoteConn { conn, holder_id } => Some((conn, holder_id)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let position = |want: &dyn Fn(&HostEvent) -> bool| events().iter().position(want);

    // Starting runs nothing of ccrpc's (it would mean a login shell at every launch).
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!dir.join("calls").exists());

    // The window connects first, then a client asks: one connection, told to the window only for the client's call.
    let h1 = host.ccrpc_connect("H1".into()).await.unwrap();
    assert!(remote_conns().is_empty());
    assert_eq!(calls(dir, "ls"), 0, "connecting needs no ls");
    assert_eq!(shared("H1").await, h1);
    assert_eq!(remote_conns(), [(h1.clone(), Some("H1".to_string()))]);
    assert_eq!(host.ccrpc_connect("H1".into()).await.unwrap(), h1);

    // The other way round.
    let h2 = shared("H2").await;
    assert_ne!(h2, h1);
    assert_eq!(host.ccrpc_connect("H2".into()).await.unwrap(), h2);
    // The processes write their call as they start, a moment after the spawn returned.
    eventually("both attaches", async || calls(dir, "attach") == 2).await;

    let snapshot: Value = serde_json::from_str(&host.connect_snapshot().await.unwrap()).unwrap();
    let snapshot = snapshot.as_array().unwrap().clone();
    assert_eq!(snapshot.len(), 2, "{snapshot:?}");
    assert!(
        snapshot
            .iter()
            .any(|s| s["conn"] == h1.as_str() && s["holderId"] == "H1"),
        "{snapshot:?}"
    );

    // A client's Disconnect is announced before the exit, so the window shows no failure.
    host.remote_call("ccrpc_disconnect", json!({ "conn": h2 }))
        .await
        .unwrap();
    let exit_of = |c: String| move |e: &HostEvent| matches!(e, HostEvent::CcrpcExit { conn, .. } if *conn == c);
    let closing_of = |c: String| move |e: &HostEvent| matches!(e, HostEvent::CcrpcClosing { conn } if *conn == c);
    eventually("the Disconnect's exit", async || {
        position(&exit_of(h2.clone())).is_some()
    })
    .await;
    let closing = position(&closing_of(h2.clone())).expect("no CcrpcClosing");
    assert!(closing < position(&exit_of(h2.clone())).unwrap());

    // A Disconnect in progress: not handed back, and the new attach waits for the old ccrpc to let go.
    host.ccrpc_disconnect(h1.clone()).await.unwrap();
    let again = shared("H1").await;
    assert_ne!(again, h1, "a dying connection was handed back");
    eventually("the new connection", async || {
        let states: Value = serde_json::from_str(&host.connect_snapshot().await.unwrap()).unwrap();
        states
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["conn"] == again.as_str() && s["ready"] == true)
    })
    .await;
    assert_eq!(calls(dir, "attach"), 3);
    // The window's own Disconnect: its own front end knows.
    assert!(position(&closing_of(h1.clone())).is_none());

    // One `ccrpc ls --watch` at a time: the window's, then the Host's own while LAN access is on.
    let lists = || {
        events()
            .iter()
            .filter(|e| matches!(e, HostEvent::CcrpcLs { .. }))
            .count()
    };
    host.ccrpc_watch_sessions(true).await.unwrap();
    eventually("the window's watch", async || lists() >= 1).await;
    assert_eq!(calls(dir, "ls"), 1);

    let on = host.lan_set_enabled(true).await.unwrap();
    assert!(on.enabled && on.addresses.is_empty() && on.error.is_none());
    let list = async || host.remote_call("ccrpc_ls", Value::Null).await;
    eventually("the Host's watch", async || list().await.is_ok()).await;
    assert_eq!(calls(dir, "ls"), 2, "the Host's watch took over");

    // The window asks while the Host keeps the watch: no restart, and the list at once.
    eventually("the Host's first list", async || lists() >= 2).await;
    let before = lists();
    host.ccrpc_watch_sessions(true).await.unwrap();
    assert_eq!(lists(), before + 1);
    assert_eq!(calls(dir, "ls"), 2);

    // LAN access off: the window, which still wants it, has its own again, and the Host's cached list is gone.
    host.lan_set_enabled(false).await.unwrap();
    eventually("the window's watch back", async || calls(dir, "ls") == 3).await;
    let err = list().await.unwrap_err();
    assert!(err.message.contains("还没有"), "{err}");

    host.shutdown();
}
