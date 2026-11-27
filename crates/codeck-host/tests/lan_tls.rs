//! End-to-end TLS + auth + pairing + revocation tests for [`TlsListener`], [`LanSecurity`], and [`router`].

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ConnectInfo;
use axum::routing::{get, post};
use codeck_host::lan::LanConfig;
use codeck_host::lan::net::Subnet;
use codeck_host::lan::pairing::PairOutcome;
use codeck_host::lan::security::LanSecurity;
use codeck_host::lan::server::{Ctx, Gate, event_channel, router, router_with_pair_timeout};
use codeck_host::lan::tls::{
    ConnLimits, IDLE_READ_TIMEOUT, MAX_CONNECTIONS, MAX_CONNECTIONS_PER_IP, TlsListener,
    cert_fingerprint, into_make_service,
};
use codeck_host::{Host, HostConfig, HostEvent};
use futures_util::StreamExt;
use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, OtherError, SignatureScheme};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

#[derive(Debug)]
struct PinnedCertVerifier {
    expected_fingerprint: String,
    algs: WebPkiSupportedAlgorithms,
}

impl PinnedCertVerifier {
    fn new(expected_fingerprint: String) -> Self {
        Self {
            expected_fingerprint,
            algs: rustls::crypto::ring::default_provider().signature_verification_algorithms,
        }
    }
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let got = cert_fingerprint(end_entity.as_ref());
        if got == self.expected_fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::InvalidCertificate(
                rustls::CertificateError::Other(OtherError(Arc::new(std::io::Error::other(
                    format!("fingerprint mismatch: {got}"),
                )))),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

fn tls_connector(fingerprint: &str) -> TlsConnector {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCertVerifier::new(fingerprint.to_owned())))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsConnector::from(Arc::new(cfg))
}

async fn connect_tls(addr: SocketAddr, fingerprint: &str) -> std::io::Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST).into());
    tls_connector(fingerprint).connect(name, tcp).await
}

async fn https_req(
    addr: SocketAddr,
    fingerprint: &str,
    head: &str,
    body: &str,
) -> (u16, String, String) {
    let mut tls = connect_tls(addr, fingerprint).await.unwrap();
    let req = format!(
        "{head}\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    tls.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    tls.read_to_string(&mut out).await.unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    let status = head[9..12].parse().unwrap();
    (status, head.to_ascii_lowercase(), body.to_string())
}

async fn connect_wss(
    addr: SocketAddr,
    fingerprint: &str,
    token: Option<&str>,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<TlsStream<TcpStream>>,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    let tls = connect_tls(addr, fingerprint).await.unwrap();
    let mut req = format!("wss://{addr}/v1/events")
        .into_client_request()
        .unwrap();
    if let Some(t) = token {
        req.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {t}")).unwrap(),
        );
    }
    tokio_tungstenite::client_async(req, tls).await
}

fn assert_unauthorized(status: u16, body: &str) {
    assert_eq!(status, 401, "{body}");
    let v: Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["error"]["code"], "unauthorized");
    assert_eq!(
        v["error"]["message"],
        "这台设备没有配对或已被移除，请重新扫码配对。"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_pairing_auth_revocation_and_reload_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("transcript.html"), "<!doctype html>").unwrap();

    let sec_dir = dir.path().join("sec");
    let security = Arc::new(LanSecurity::open(sec_dir.clone()).unwrap());
    let cert = security.ensure_cert().unwrap();
    let fp = cert.fingerprint.clone();

    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let lan_port = free.local_addr().unwrap().port();
    drop(free);

    let host_state = dir.path().join("host-state");
    let (events, lan_sink) = event_channel();
    let captured: Arc<Mutex<Vec<HostEvent>>> = Arc::default();
    let sink = {
        let captured = Arc::clone(&captured);
        Arc::new(move |e: HostEvent| {
            lan_sink(e.clone());
            captured.lock().push(e);
        })
    };
    let host = Host::start(
        HostConfig {
            cache_dir: host_state.join("cache"),
            settings_path: host_state.join("settings.json"),
            ccrpc_logs: Some(host_state.join("logs")),
            images: Some(host_state.join("images")),
            data_root: Some(dir.path().join("root")),
            refresh_prices: false,
            lan: Some(LanConfig {
                events: events.clone(),
                web_dir: web.clone(),
                port: lan_port,
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

    // Cert is not created until LAN access is turned on; broken `tls/` errors and recovers once fixed.
    assert!(!host_state.join("tls").exists());
    std::fs::create_dir_all(host_state.join("tls")).unwrap();
    std::fs::write(host_state.join("tls/cert.pem"), "not a cert").unwrap();
    let bad = host.lan_set_enabled(true).await.unwrap();
    assert!(bad.enabled && bad.addresses.is_empty());
    assert!(
        bad.error.as_deref().is_some_and(|e| e.contains("私钥")),
        "{bad:?}"
    );
    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, lan_port))
            .await
            .is_err()
    );
    host.lan_set_enabled(false).await.unwrap();
    std::fs::remove_dir_all(host_state.join("tls")).unwrap();
    let ok = host.lan_set_enabled(true).await.unwrap();
    assert_eq!(ok.error, None);
    assert_eq!(ok.addresses, [format!("127.0.0.1:{lan_port}")]);
    host.lan_set_enabled(false).await.unwrap();

    // Bind a forced-TLS + auth listener directly on 127.0.0.1.
    let subnet = Subnet {
        addr: Ipv4Addr::LOCALHOST,
        mask: Ipv4Addr::new(255, 0, 0, 0),
    };
    let ctx = Ctx {
        host,
        events: events.clone(),
        allowed_origins: Arc::new(vec!["http://localhost:1420".into()]),
        web_dir: web,
        gate: Gate::default(),
        security: Some(Arc::clone(&security)),
    };
    let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let listener = TlsListener::new(tcp, cert.server_config());
    let app = into_make_service(router(ctx, subnet).route(
        "/test-peer",
        get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.ip().to_string() }),
    ));
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // 1. Public `/health` (GET & HEAD) works without a token and reports apiVersion 2; TLS same-origin uses https://.
    let (status, _, body) = https_req(addr, &fp, "GET /health HTTP/1.1", "").await;
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["product"], "CoDeck");
    assert_eq!(v["apiVersion"], 2);

    let (status, _, body) = https_req(addr, &fp, "HEAD /health HTTP/1.1", "").await;
    assert_eq!((status, body.as_str()), (200, ""));

    let (status, ..) = https_req(
        addr,
        &fp,
        &format!("GET /health HTTP/1.1\r\nOrigin: https://{addr}"),
        "",
    )
    .await;
    assert_eq!(status, 200);
    let (status, ..) = https_req(
        addr,
        &fp,
        &format!("GET /health HTTP/1.1\r\nOrigin: http://{addr}"),
        "",
    )
    .await;
    assert_eq!(status, 403);

    // Allowed Origin OPTIONS preflight stays 204 without auth; actual POST still requires auth.
    let (status, head, _) = https_req(
        addr,
        &fp,
        "OPTIONS /v1/rpc HTTP/1.1\r\nOrigin: http://localhost:1420\r\nAccess-Control-Request-Method: POST",
        "",
    )
    .await;
    assert_eq!(status, 204);
    assert!(head.contains("access-control-allow-origin: http://localhost:1420"));

    // 2. Unauthenticated or bad-token requests to every protected endpoint return 401 `unauthorized`,
    //    and `/v1/rpc` / `/v1/pair` always carry `Cache-Control: no-store` even on middleware rejections.
    for auth in ["", "\r\nAuthorization: Bearer invalid-token"] {
        let (status, head, body) = https_req(
            addr,
            &fp,
            &format!("POST /v1/rpc HTTP/1.1\r\nContent-Type: application/json{auth}"),
            r#"{"method":"list_projects"}"#,
        )
        .await;
        assert_unauthorized(status, &body);
        assert!(head.contains("cache-control: no-store"), "{head}");

        for req in [
            "GET /web/transcript.html HTTP/1.1",
            "HEAD /web/transcript.html HTTP/1.1",
            "GET /web/ HTTP/1.1",
            "GET /web HTTP/1.1",
            "POST /health HTTP/1.1",
            "GET /v1/pair HTTP/1.1",
        ] {
            let (status, _, body) = https_req(addr, &fp, &format!("{req}{auth}"), "").await;
            if req.starts_with("HEAD ") {
                assert_eq!(status, 401, "{req}");
            } else {
                assert_unauthorized(status, &body);
            }
        }
    }

    // Forbidden Origin on `/v1/pair` and `/v1/rpc` still carries `Cache-Control: no-store`.
    for path in ["/v1/pair", "/v1/rpc"] {
        let (status, head, _) = https_req(
            addr,
            &fp,
            &format!(
                "POST {path} HTTP/1.1\r\nOrigin: https://evil.example\r\nContent-Type: application/json"
            ),
            "{}",
        )
        .await;
        assert_eq!(status, 403, "{path}");
        assert!(head.contains("cache-control: no-store"), "{path}: {head}");
    }

    // WSS without token or with wrong token is rejected with HTTP 401 before upgrade.
    for tok in [None, Some("wrong-token")] {
        match connect_wss(addr, &fp, tok).await.unwrap_err() {
            tokio_tungstenite::tungstenite::Error::Http(resp) => assert_eq!(resp.status(), 401),
            other => panic!("expected 401, got {other:?}"),
        }
    }

    // 3. `/v1/pair` validation, persistence failure recovery, redemption, event emission, single-use.
    let (status, head, _) = https_req(addr, &fp, "POST /v1/pair HTTP/1.1", r#"{"key":"k"}"#).await;
    assert_eq!(status, 415);
    assert!(head.contains("cache-control: no-store"), "{head}");

    let big = format!(r#"{{"key":"k","name":"{}"}}"#, "x".repeat(5000));
    let (status, head, _) = https_req(
        addr,
        &fp,
        "POST /v1/pair HTTP/1.1\r\nContent-Type: application/json",
        &big,
    )
    .await;
    assert_eq!(status, 413);
    assert!(head.contains("cache-control: no-store"), "{head}");

    let (status, head, body) = https_req(
        addr,
        &fp,
        "POST /v1/pair HTTP/1.1\r\nContent-Type: application/json",
        r#"{"key":"no-such-key","name":"iPhone"}"#,
    )
    .await;
    assert_eq!(status, 403);
    assert!(head.contains("cache-control: no-store"), "{head}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "pairing_expired");
    assert_eq!(
        v["error"]["message"],
        "配对码已过期或已用过，请在 Host 上重新生成。"
    );

    // Save failure returns 500 `pairing_failed` and leaves the offer valid for retry.
    let mut offer_a = security.start_pairing().unwrap();
    let blocker = sec_dir.join("devices.json");
    std::fs::create_dir(&blocker).unwrap();
    let pair_a_body = json!({ "key": offer_a.key, "name": "  iPhone A  " }).to_string();
    let (status, head, body) = https_req(
        addr,
        &fp,
        "POST /v1/pair HTTP/1.1\r\nContent-Type: application/json",
        &pair_a_body,
    )
    .await;
    assert_eq!(status, 500);
    assert!(head.contains("cache-control: no-store"), "{head}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "pairing_failed");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("保存配对失败："),
        "{v}"
    );
    std::fs::remove_dir(&blocker).unwrap();

    // Retry with the same key now succeeds, emits `LanDevices`, and notifies the offer's oneshot.
    captured.lock().clear();
    let (status, head, body) = https_req(
        addr,
        &fp,
        "POST /v1/pair HTTP/1.1\r\nContent-Type: application/json",
        &pair_a_body,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(head.contains("cache-control: no-store"), "{head}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let dev_a_id = v["result"]["deviceId"].as_str().unwrap().to_owned();
    let token_a = v["result"]["token"].as_str().unwrap().to_owned();
    assert_eq!(dev_a_id.len(), 12);
    assert_eq!(token_a.len(), 43);

    match offer_a.outcome.try_recv().unwrap() {
        PairOutcome::Paired(dev) => {
            assert_eq!(dev.id, dev_a_id);
            assert_eq!(dev.name, "iPhone A");
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
    let lan_dev_event = captured
        .lock()
        .iter()
        .find_map(|e| match e {
            HostEvent::LanDevices {
                devices,
                paired_offer,
                paired_device,
            } => Some((devices.clone(), paired_offer.clone(), paired_device.clone())),
            _ => None,
        })
        .expect("HostEvent::LanDevices was not emitted");
    assert_eq!(lan_dev_event.0.len(), 1);
    assert_eq!(lan_dev_event.1.as_deref(), Some(offer_a.id.as_str()));
    assert_eq!(lan_dev_event.2.as_deref(), Some(dev_a_id.as_str()));

    // Reusing the same key fails with 403 `pairing_expired`.
    let (status, _, body) = https_req(
        addr,
        &fp,
        "POST /v1/pair HTTP/1.1\r\nContent-Type: application/json",
        &pair_a_body,
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["error"]["code"],
        "pairing_expired"
    );

    // 4. Valid token accesses `/test-peer` (ConnectInfo = 127.0.0.1), `/v1/rpc`, `/web/*`, and WSS.
    let (status, _, peer_ip) = https_req(
        addr,
        &fp,
        &format!("GET /test-peer HTTP/1.1\r\nAuthorization: Bearer {token_a}"),
        "",
    )
    .await;
    assert_eq!((status, peer_ip.as_str()), (200, "127.0.0.1"));

    let (status, head, body) = https_req(
        addr,
        &fp,
        &format!(
            "POST /v1/rpc HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer {token_a}"
        ),
        r#"{"method":"list_projects"}"#,
    )
    .await;
    assert_eq!(status, 200);
    assert!(head.contains("cache-control: no-store"), "{head}");
    assert!(serde_json::from_str::<Value>(&body).unwrap()["result"].is_array());

    // Local methods remain blocked over RPC even when authenticated.
    let (status, _, body) = https_req(
        addr,
        &fp,
        &format!(
            "POST /v1/rpc HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer {token_a}"
        ),
        r#"{"method":"lan_status"}"#,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["error"]["code"],
        "invalidQuery"
    );

    let (status, _, body) = https_req(
        addr,
        &fp,
        &format!("GET /web/transcript.html HTTP/1.1\r\nAuthorization: Bearer {token_a}"),
        "",
    )
    .await;
    assert_eq!((status, body.as_str()), (200, "<!doctype html>"));
    let (status, ..) = https_req(
        addr,
        &fp,
        &format!("HEAD /web/transcript.html HTTP/1.1\r\nAuthorization: Bearer {token_a}"),
        "",
    )
    .await;
    assert_eq!(status, 200);

    // Pair a second device B to confirm revoking A does not affect B.
    let offer_b = security.start_pairing().unwrap();
    let (status, _, body) = https_req(
        addr,
        &fp,
        "POST /v1/pair HTTP/1.1\r\nContent-Type: application/json",
        &json!({ "key": offer_b.key, "name": "iPhone B" }).to_string(),
    )
    .await;
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    let dev_b_id = v["result"]["deviceId"].as_str().unwrap().to_owned();
    let token_b = v["result"]["token"].as_str().unwrap().to_owned();

    // Device A opens two WSS connections (one idle, one stalled on reads while flooded) + one slow HTTP body.
    captured.lock().clear();
    let (mut ws_a1, resp) = connect_wss(addr, &fp, Some(&token_a)).await.unwrap();
    assert_eq!(resp.status(), 101);
    let first = ws_a1.next().await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(first.to_text().unwrap()).unwrap(),
        json!({ "event": "hello", "payload": { "apiVersion": 2 } })
    );
    // Connecting WSS touched `lastSeenAt` and emitted `HostEvent::LanDevices` with null pair fields.
    assert!(
        security
            .devices()
            .iter()
            .find(|d| d.id == dev_a_id)
            .unwrap()
            .last_seen_at
            .is_some()
    );
    let touch_ev = captured
        .lock()
        .iter()
        .find_map(|e| match e {
            HostEvent::LanDevices {
                devices,
                paired_offer,
                paired_device,
            } => Some((devices.clone(), paired_offer.clone(), paired_device.clone())),
            _ => None,
        })
        .expect("HostEvent::LanDevices was not emitted on WS touch");
    assert_eq!(touch_ev.1, None);
    assert_eq!(touch_ev.2, None);
    assert!(
        touch_ev
            .0
            .iter()
            .find(|d| d.id == dev_a_id)
            .unwrap()
            .last_seen_at
            .is_some()
    );

    let (mut ws_a2, _) = connect_wss(addr, &fp, Some(&token_a)).await.unwrap();
    let _ = ws_a2.next().await.unwrap().unwrap();

    let (mut ws_b, _) = connect_wss(addr, &fp, Some(&token_b)).await.unwrap();
    let _ = ws_b.next().await.unwrap().unwrap();

    // Slow request body in flight for Device A.
    let mut slow_a = connect_tls(addr, &fp).await.unwrap();
    slow_a
        .write_all(
            format!(
                "POST /v1/rpc HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token_a}\r\nContent-Type: application/json\r\nContent-Length: 32\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    slow_a.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Flood events while ws_a2 stops reading (saturates server send path); ws_b drains so it doesn't lag.
    let big_event: axum::extract::ws::Utf8Bytes =
        json!({ "event": "test-fill", "payload": "x".repeat(64 * 1024) })
            .to_string()
            .into();
    for _ in 0..80 {
        let _ = events.send(big_event.clone());
    }
    for _ in 0..80 {
        let _ = ws_b.next().await.unwrap().unwrap();
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 5. Revoke Device A: both A WebSockets disconnect, slow-body HTTP returns 401 & closes, B stays alive!
    let (removed, remaining) = security.remove(&dev_a_id).unwrap();
    assert!(removed);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, dev_b_id);

    // Slow HTTP body receives 401 with `Cache-Control: no-store` and server closes the connection.
    let mut slow_out = String::new();
    tokio::time::timeout(Duration::from_secs(3), slow_a.read_to_string(&mut slow_out))
        .await
        .expect("slow request did not receive 401 on revocation")
        .unwrap();
    let (slow_head, slow_body) = slow_out.split_once("\r\n\r\n").unwrap();
    assert!(slow_head.starts_with("HTTP/1.1 401"), "{slow_head}");
    assert!(
        slow_head
            .to_ascii_lowercase()
            .contains("cache-control: no-store"),
        "{slow_head}"
    );
    assert_unauthorized(401, slow_body);

    // Both A WebSockets terminate promptly (after draining any already-buffered frames).
    for mut ws in [ws_a1, ws_a2] {
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(Ok(_)) = ws.next().await {}
        })
        .await
        .expect("revoked device WebSocket stayed open");
    }

    // Device A's subsequent requests fail with 401.
    let (status, _, body) = https_req(
        addr,
        &fp,
        &format!(
            "POST /v1/rpc HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer {token_a}"
        ),
        r#"{"method":"list_projects"}"#,
    )
    .await;
    assert_unauthorized(status, &body);

    // Device B still receives events and makes RPC calls.
    let ping: axum::extract::ws::Utf8Bytes = json!({ "event": "after-revoke", "payload": 1 })
        .to_string()
        .into();
    let _ = events.send(ping);
    let got_b = tokio::time::timeout(Duration::from_secs(3), ws_b.next())
        .await
        .expect("device B stopped receiving events")
        .unwrap()
        .unwrap();
    assert!(got_b.to_text().unwrap().contains("after-revoke"));

    let (status, ..) = https_req(
        addr,
        &fp,
        &format!(
            "POST /v1/rpc HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer {token_b}"
        ),
        r#"{"method":"list_projects"}"#,
    )
    .await;
    assert_eq!(status, 200);
    drop(ws_b);

    // 6. Plaintext, wrong fingerprint, and stalled peer isolation.
    let mut plain = TcpStream::connect(addr).await.unwrap();
    let _ = plain
        .write_all(
            format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await;
    let mut plain_buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), plain.read_to_end(&mut plain_buf)).await;
    assert!(
        !String::from_utf8_lossy(&plain_buf).starts_with("HTTP/1.1 200"),
        "plaintext unexpectedly succeeded"
    );

    let wrong_fp = "A".repeat(43);
    assert_ne!(wrong_fp, fp);
    assert!(connect_tls(addr, &wrong_fp).await.is_err());

    let _stalled = TcpStream::connect(addr).await.unwrap();
    let (status, ..) = tokio::time::timeout(
        Duration::from_secs(3),
        https_req(addr, &fp, "GET /health HTTP/1.1", ""),
    )
    .await
    .expect("stalled peer blocked concurrent TLS client");
    assert_eq!(status, 200);

    // 7. Opening a second LanSecurity on the same directory fails while the first holds `daemon.lock`.
    let lock_err = LanSecurity::open(sec_dir.clone()).err().unwrap();
    assert!(
        lock_err.contains("另一个 codeck-host 正在使用"),
        "{lock_err}"
    );

    // 8. Dropping the server task frees the port; once `security` drops, reloading preserves
    //    certificate fingerprint and Device B and rejects removed Device A.
    server_task.abort();
    let _ = server_task.await;
    drop(security);

    let reloaded = LanSecurity::open(sec_dir).unwrap();
    assert_eq!(reloaded.ensure_cert().unwrap().fingerprint, fp);
    assert_eq!(reloaded.verify(&token_b).unwrap().0, dev_b_id);
    assert!(reloaded.verify(&token_a).is_none());

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpListener::bind(addr).await.is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "port {addr} was not freed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_per_ip_connection_limit_and_slow_pair_body_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let sec = Arc::new(LanSecurity::open(dir.path().join("sec")).unwrap());
    let cert = sec.ensure_cert().unwrap();
    let fp = cert.fingerprint.clone();

    // 1. Per-IP limit (2) rejects a 3rd connection from 127.0.0.1 while another source IP (::1)
    //    still connects and succeeds, and dropping a held connection frees the slot.
    let dual_tcp = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0)).await.unwrap();
    let port = dual_tcp.local_addr().unwrap().port();
    let v4_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let v6_addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port);

    let limited_listener = TlsListener::with_limits(dual_tcp, cert.server_config(), 16, 2);
    let app = axum::Router::new().route("/ping", get(|| async { "ok" }));
    let limit_srv = tokio::spawn(async move {
        let _ = axum::serve(limited_listener, app).await;
    });

    // Hold 2 TLS connections open from 127.0.0.1.
    let c1 = connect_tls(v4_addr, &fp).await.unwrap();
    let c2 = connect_tls(v4_addr, &fp).await.unwrap();

    // 3rd connection from the same source IP is dropped before handshake.
    let third = tokio::time::timeout(Duration::from_secs(2), connect_tls(v4_addr, &fp))
        .await
        .expect("3rd connection should be rejected promptly");
    assert!(
        third.is_err(),
        "expected 3rd connection from same IP to fail"
    );

    // A different source IP (::1) still completes TLS handshake and HTTP request normally.
    let mut other_ip = {
        let tcp = TcpStream::connect(v6_addr).await.unwrap();
        let name = ServerName::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST).into());
        tls_connector(&fp).connect(name, tcp).await.unwrap()
    };
    other_ip
        .write_all(
            format!("GET /ping HTTP/1.1\r\nHost: [::1]:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut resp = String::new();
    other_ip.read_to_string(&mut resp).await.unwrap();
    assert!(
        resp.starts_with("HTTP/1.1 200") && resp.ends_with("ok"),
        "{resp}"
    );
    drop(other_ip);

    // Closing one held 127.0.0.1 connection releases its permit so a new one succeeds.
    drop(c1);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(_c3) = connect_tls(v4_addr, &fp).await {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "permit was not released after drop"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(c2);
    limit_srv.abort();
    let _ = limit_srv.await;

    // 2. Slow `/v1/pair` request body is timed out with 408 + `Cache-Control: no-store` and disconnected.
    let (events, sink) = event_channel();
    let host_state = dir.path().join("host-state");
    let host = Host::start(
        HostConfig {
            cache_dir: host_state.join("cache"),
            settings_path: host_state.join("settings.json"),
            ccrpc_logs: Some(host_state.join("logs")),
            images: Some(host_state.join("images")),
            data_root: Some(dir.path().join("root")),
            refresh_prices: false,
            ..HostConfig::desktop()
        },
        sink,
    )
    .unwrap();
    let ctx = Ctx {
        host,
        events,
        allowed_origins: Arc::default(),
        web_dir: dir.path().join("web"),
        gate: Gate::default(),
        security: Some(Arc::clone(&sec)),
    };
    let subnet = Subnet {
        addr: Ipv4Addr::LOCALHOST,
        mask: Ipv4Addr::new(255, 0, 0, 0),
    };
    let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let pair_addr = tcp.local_addr().unwrap();
    let listener = TlsListener::new(tcp, cert.server_config());
    let pair_app = into_make_service(router_with_pair_timeout(
        ctx,
        subnet,
        Duration::from_millis(250),
    ));
    let pair_srv = tokio::spawn(async move {
        let _ = axum::serve(listener, pair_app).await;
    });

    let mut slow = connect_tls(pair_addr, &fp).await.unwrap();
    slow.write_all(
        format!(
            "POST /v1/pair HTTP/1.1\r\nHost: {pair_addr}\r\nContent-Type: application/json\r\nContent-Length: 64\r\n\r\n{{\"key"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    slow.flush().await.unwrap();

    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(3), slow.read_to_string(&mut out))
        .await
        .expect("slow /v1/pair body was not timed out and closed")
        .unwrap();
    let lower = out.to_ascii_lowercase();
    assert!(out.starts_with("HTTP/1.1 408"), "{out}");
    assert!(lower.contains("cache-control: no-store"), "{out}");
    assert!(lower.contains("connection: close"), "{out}");

    pair_srv.abort();
    let _ = pair_srv.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_shared_quota_across_two_listeners_and_idle_read_timeout() {
    assert_eq!(MAX_CONNECTIONS, 128);
    assert_eq!(MAX_CONNECTIONS_PER_IP, 16);
    assert_eq!(IDLE_READ_TIMEOUT, Duration::from_secs(180));

    let dir = tempfile::tempdir().unwrap();
    let sec = Arc::new(LanSecurity::open(dir.path().join("sec")).unwrap());
    let cert = sec.ensure_cert().unwrap();
    let fp = cert.fingerprint.clone();

    // 1. Two listeners sharing one ConnLimits budget (total 3, per-IP 2).
    let shared = ConnLimits::new(3, 2);
    let tcp1 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr1 = tcp1.local_addr().unwrap();
    let tcp2 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr2 = tcp2.local_addr().unwrap();

    let l1 = TlsListener::with_shared_limits(
        tcp1,
        cert.server_config(),
        Arc::clone(&shared),
        IDLE_READ_TIMEOUT,
    );
    let l2 = TlsListener::with_shared_limits(
        tcp2,
        cert.server_config(),
        Arc::clone(&shared),
        IDLE_READ_TIMEOUT,
    );
    let srv1 = tokio::spawn(async move {
        let app = axum::Router::new().route("/ping", get(|| async { "ok1" }));
        let _ = axum::serve(l1, app).await;
    });
    let srv2 = tokio::spawn(async move {
        let app = axum::Router::new().route("/ping", get(|| async { "ok2" }));
        let _ = axum::serve(l2, app).await;
    });

    // 1st connection on listener 1 + 2nd on listener 2 exhaust the shared per-IP quota (2).
    let c1 = connect_tls(addr1, &fp).await.unwrap();
    let c2 = connect_tls(addr2, &fp).await.unwrap();
    let third_on_1 = tokio::time::timeout(Duration::from_secs(2), connect_tls(addr1, &fp))
        .await
        .expect("3rd connection on listener 1 should fail promptly");
    assert!(third_on_1.is_err());
    let third_on_2 = tokio::time::timeout(Duration::from_secs(2), connect_tls(addr2, &fp))
        .await
        .expect("3rd connection on listener 2 should fail promptly");
    assert!(third_on_2.is_err());

    // Dropping the connection on listener 1 frees budget for listener 2.
    drop(c1);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(_c3) = connect_tls(addr2, &fp).await {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "shared permit was not released across listeners"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(c2);
    srv1.abort();
    srv2.abort();
    let _ = srv1.await;
    let _ = srv2.await;

    // 2. Idle read timeout (short injectable 250ms): silent connection is closed, stalled request
    //    body is closed, active reads reset it, and slow in-flight handlers (> 250ms) still return
    //    200 without being disconnected.
    let idle_tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let idle_addr = idle_tcp.local_addr().unwrap();
    let idle_listener = TlsListener::with_shared_limits(
        idle_tcp,
        cert.server_config(),
        ConnLimits::new(16, 16),
        Duration::from_millis(250),
    );
    let idle_srv = tokio::spawn(async move {
        let app = axum::Router::new()
            .route("/ping", get(|| async { "ok" }))
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    "slow-ok"
                }),
            )
            .route(
                "/slow-rpc",
                post(|body: Bytes| async move {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    format!("len={}", body.len())
                }),
            );
        let _ = axum::serve(idle_listener, into_make_service(app)).await;
    });

    // Case A: connection sends nothing after TLS handshake -> closed by server after idle timeout.
    let mut silent = connect_tls(idle_addr, &fp).await.unwrap();
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(2), silent.read(&mut buf))
        .await
        .expect("idle TLS connection was not closed after idle read timeout");
    assert!(
        matches!(n, Ok(0) | Err(_)),
        "expected EOF or error on idle close, got {n:?}"
    );

    // Case B: periodic reads (< 250ms apart) keep the connection alive past 250ms, then idling closes it.
    let mut active = connect_tls(idle_addr, &fp).await.unwrap();
    let req = format!("GET /ping HTTP/1.1\r\nHost: {idle_addr}\r\nConnection: keep-alive\r\n\r\n");
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        active.write_all(req.as_bytes()).await.unwrap();
        active.flush().await.unwrap();
        let mut resp = [0u8; 256];
        let n = tokio::time::timeout(Duration::from_secs(2), active.read(&mut resp))
            .await
            .expect("active connection timed out unexpectedly")
            .unwrap();
        let text = String::from_utf8_lossy(&resp[..n]);
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    }
    // Now stop sending; server must close the idle keep-alive connection.
    let n = tokio::time::timeout(Duration::from_secs(2), active.read(&mut buf))
        .await
        .expect("keep-alive connection was not closed after going idle");
    assert!(
        matches!(n, Ok(0) | Err(_)),
        "expected EOF or error after going idle, got {n:?}"
    );

    // Case C: in-flight GET and POST handlers slower than the 250ms idle timeout still return 200,
    // and going idle afterward on keep-alive still closes the connection.
    let mut slow_get = connect_tls(idle_addr, &fp).await.unwrap();
    slow_get
        .write_all(
            format!("GET /slow HTTP/1.1\r\nHost: {idle_addr}\r\nConnection: keep-alive\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    slow_get.flush().await.unwrap();
    let mut resp = [0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(3), slow_get.read(&mut resp))
        .await
        .expect("slow GET handler was prematurely disconnected")
        .unwrap();
    let text = String::from_utf8_lossy(&resp[..n]);
    assert!(
        text.starts_with("HTTP/1.1 200") && text.contains("slow-ok"),
        "{text}"
    );
    let n = tokio::time::timeout(Duration::from_secs(2), slow_get.read(&mut buf))
        .await
        .expect("keep-alive connection was not closed after slow handler finished");
    assert!(
        matches!(n, Ok(0) | Err(_)),
        "expected EOF or error after slow handler went idle, got {n:?}"
    );

    let (status, _, body) = tokio::time::timeout(
        Duration::from_secs(3),
        https_req(
            idle_addr,
            &fp,
            "POST /slow-rpc HTTP/1.1\r\nContent-Type: application/json",
            r#"{"method":"connect_command"}"#,
        ),
    )
    .await
    .expect("slow POST handler was prematurely disconnected");
    assert_eq!((status, body.as_str()), (200, "len=28"));

    // Case D: stalled incomplete POST request body is NOT paused and still times out and closes
    // (axum may emit a 400/408 error response when body read fails, then closes the connection).
    let mut stalled_body = connect_tls(idle_addr, &fp).await.unwrap();
    stalled_body
        .write_all(
            format!(
                "POST /slow-rpc HTTP/1.1\r\nHost: {idle_addr}\r\nContent-Length: 64\r\n\r\n{{\"a\":"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stalled_body.flush().await.unwrap();
    let mut stalled_out = Vec::new();
    let res = tokio::time::timeout(
        Duration::from_secs(2),
        stalled_body.read_to_end(&mut stalled_out),
    )
    .await
    .expect("stalled request body was not closed by idle timeout");
    let text = String::from_utf8_lossy(&stalled_out);
    assert!(
        !text.starts_with("HTTP/1.1 200"),
        "stalled body unexpectedly succeeded: {res:?} {text}"
    );

    idle_srv.abort();
    let _ = idle_srv.await;
}

#[test]
fn cli_rejects_empty_root_and_untrusted_state_dir() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let bin = env!("CARGO_BIN_EXE_codeck-host");

    // Empty, `/`, and `//` must fail with exit code 1 for daemon and all CLI subcommands.
    let cases: &[&[&str]] = &[
        &["--state-dir", ""],
        &["--state-dir", "/"],
        &["--state-dir", "//"],
        &["pair", "--state-dir", ""],
        &["devices", "--state-dir", "/"],
        &["unpair", "abc123", "--state-dir", "//"],
    ];
    for args in cases {
        let out = Command::new(bin).args(*args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "args={args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("状态目录不能是空路径或根目录"),
            "args={args:?}, stderr={stderr}"
        );
    }

    // 0777 ancestor without sticky bit must be rejected with exit code 1 naming that ancestor.
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("world");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
    let bad_state = shared.join("state");
    let out = Command::new(bin)
        .args(["--listen", "127.0.0.1", "--state-dir"])
        .arg(&bad_state)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("sticky"), "{stderr}");
    assert!(stderr.contains(&shared.display().to_string()), "{stderr}");
    assert!(!bad_state.exists());

    // Group-writable settings.json in daemon state dir must be rejected with exit code 1.
    let ok_state = dir.path().join("daemon-state");
    std::fs::create_dir(&ok_state).unwrap();
    let settings = ok_state.join("settings.json");
    std::fs::write(&settings, "{}").unwrap();
    std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o660)).unwrap();
    let out = Command::new(bin)
        .args(["--listen", "127.0.0.1", "--state-dir"])
        .arg(&ok_state)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("组或其他用户写入"), "{stderr}");
}
