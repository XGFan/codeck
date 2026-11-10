//! 「局域网访问」 switched on and off, on loopback. One test: the settings path is process-wide.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use codeck_host::lan::LanConfig;
use codeck_host::lan::net::Subnet;
use codeck_host::lan::server::event_channel;
use codeck_host::{Host, HostConfig, settings};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

/// One request on `s`, left open; the status and the body.
async fn exchange(s: &mut TcpStream, addr: SocketAddr, head: &str, body: &str) -> (u16, String) {
    let req = format!(
        "{head}\r\nHost: {addr}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut got = Vec::new();
    let end = loop {
        if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        assert_ne!(s.read_buf(&mut got).await.unwrap(), 0, "closed early");
    };
    let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
    let len: usize = head
        .split_once("content-length: ")
        .and_then(|(_, v)| v.split("\r\n").next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while got.len() < end + len {
        assert_ne!(s.read_buf(&mut got).await.unwrap(), 0, "closed early");
    }
    (
        head[9..12].parse().unwrap(),
        String::from_utf8_lossy(&got[end..end + len]).into_owned(),
    )
}

async fn get(addr: SocketAddr, path: &str) -> u16 {
    let mut s = TcpStream::connect(addr).await.unwrap();
    exchange(&mut s, addr, &format!("GET {path} HTTP/1.1"), "")
        .await
        .0
}

async fn rpc(addr: SocketAddr, method: &str) -> Value {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let (status, body) = exchange(
        &mut s,
        addr,
        "POST /v1/rpc HTTP/1.1\r\nContent-Type: application/json",
        &json!({ "method": method }).to_string(),
    )
    .await;
    assert_eq!(status, 200);
    serde_json::from_str(&body).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn lan_access_turns_on_and_off_and_cuts_what_was_connected() {
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("transcript.html"), "<!doctype html>").unwrap();
    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = free.local_addr().unwrap();
    drop(free);
    let (events, sink) = event_channel();
    let host = Host::start(
        HostConfig {
            cache_dir: dir.path().join("cache"),
            settings_path: dir.path().join("settings.json"),
            ccrpc_logs: Some(dir.path().join("logs")),
            images: Some(dir.path().join("images")),
            data_root: Some(dir.path().join("root")),
            refresh_prices: false,
            lan: Some(LanConfig {
                events,
                web_dir: web,
                port: addr.port(),
                subnets: Arc::new(|| {
                    Ok(vec![Subnet {
                        addr: Ipv4Addr::LOCALHOST,
                        mask: Ipv4Addr::new(255, 0, 0, 0),
                    }])
                }),
            }),
            ..HostConfig::desktop()
        },
        sink,
    )
    .unwrap();
    let here = vec![addr.to_string()];

    // Off by default: nothing listens.
    assert!(!host.lan_enabled());
    assert!(!host.lan_status().await.unwrap().enabled);
    assert!(TcpStream::connect(addr).await.is_err());

    let on = host.lan_set_enabled(true).await.unwrap();
    assert!(on.enabled && host.lan_enabled());
    assert_eq!((on.addresses, on.error), (here.clone(), None));
    assert!(settings::load().lan_access);
    assert_eq!(get(addr, "/health").await, 200);
    assert_eq!(get(addr, "/web/transcript.html").await, 200);
    assert!(rpc(addr, "list_projects").await["result"].is_array());
    let local = rpc(addr, "lan_status").await;
    assert_eq!(local["error"]["code"], "invalidQuery", "{local}");

    // What is connected when it goes off: a keep-alive connection and a WebSocket.
    let mut kept = TcpStream::connect(addr).await.unwrap();
    assert_eq!(
        exchange(&mut kept, addr, "GET /health HTTP/1.1", "")
            .await
            .0,
        200
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/events"))
        .await
        .unwrap();
    let hello = ws.next().await.unwrap().unwrap();
    assert!(hello.to_text().unwrap().contains("hello"));
    // And a request past the check that has not sent its body yet.
    let mut pending = TcpStream::connect(addr).await.unwrap();
    pending
        .write_all(
            format!(
                "POST /v1/rpc HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let off = host.lan_set_enabled(false).await.unwrap();
    assert!(!off.enabled && off.addresses.is_empty() && !host.lan_enabled());
    assert!(!settings::load().lan_access);
    assert!(TcpStream::connect(addr).await.is_err(), "port still open");
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), pending.read_to_end(&mut answer))
        .await
        .expect("the pending request was left to wait for its body")
        .unwrap();
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    match ws.next().await.unwrap().unwrap() {
        Message::Close(Some(f)) => assert_eq!(u16::from(f.code), 1001),
        other => panic!("{other:?}"),
    }
    // Gone with its listener, or at most refused (`lan::server` pins the refusal); it must not answer any more.
    let _ = kept
        .write_all(format!("GET /health HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes())
        .await;
    let mut rest = Vec::new();
    let _ = kept.read_to_end(&mut rest).await;
    let rest = String::from_utf8_lossy(&rest);
    assert!(
        rest.is_empty() || rest.starts_with("HTTP/1.1 403"),
        "{rest}"
    );

    // The port is taken: still on, says why, and takes the port once it is free.
    let taken = std::net::TcpListener::bind(addr).unwrap();
    let on = host.lan_set_enabled(true).await.unwrap();
    assert!(on.enabled && on.addresses.is_empty());
    assert_eq!(on.error, Some(format!("{addr} 已被占用")));
    drop(taken);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let s = host.lan_status().await.unwrap();
        if s.addresses == here {
            assert_eq!(s.error, None);
            break;
        }
        assert!(Instant::now() < deadline, "{s:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(get(addr, "/health").await, 200);

    host.lan_set_enabled(false).await.unwrap();
    assert!(TcpStream::connect(addr).await.is_err());

    // Started with it saved as on (the settings file of a previous run).
    settings::update(|s| s.lan_access = true).unwrap();
    host.lan_restore().await;
    assert!(host.lan_enabled());
    assert_eq!(get(addr, "/health").await, 200);
    host.lan_set_enabled(false).await.unwrap();
}
