//! The HTTP + WebSocket face of the Host: `/health`, `POST /v1/rpc`, `GET /v1/events`, `/web/*`.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use codeck_core::model::{AppError, ErrorCode};
use codeck_host::{EventSink, Host, HostEvent, dispatch};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::net::{OriginCheck, Subnet, check_origin, host_allowed, peer_allowed};

/// Bumped only on incompatible changes to the methods, events or envelopes.
pub const API_VERSION: u32 = 1;
const BODY_LIMIT: usize = 32 * 1024 * 1024;
/// Events a slow client may fall behind by before it is dropped (it resyncs on reconnect).
const EVENT_BACKLOG: usize = 1024;
/// The Transcript page's policy: only this daemon's scripts, styles, images and connections (the Shiki
/// worker is a blob), never framed.
const WEB_CSP: &str = "default-src 'self'; script-src 'self'; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

#[derive(Clone)]
pub struct Ctx {
    pub host: Arc<Host>,
    pub events: broadcast::Sender<Utf8Bytes>,
    pub allowed_origins: Arc<Vec<String>>,
    pub web_dir: PathBuf,
}

/// The broadcast channel the WebSocket clients read, and the Host's sink writing to it: every event as
/// `{event, payload}`, except ccrpc's raw lines and exits (remote clients get the Connect state instead:
/// `connect-state-event`, `connect-removed-event`).
pub fn event_channel() -> (broadcast::Sender<Utf8Bytes>, EventSink) {
    let (tx, _) = broadcast::channel(EVENT_BACKLOG);
    let sink_tx = tx.clone();
    let sink: EventSink = Arc::new(move |e: HostEvent| {
        if matches!(e, HostEvent::CcrpcLine { .. } | HostEvent::CcrpcExit { .. }) {
            return;
        }
        let msg = json!({ "event": e.name(), "payload": e.payload() }).to_string();
        let _ = sink_tx.send(msg.into());
    });
    (tx, sink)
}

/// The listeners being served, one per [`Subnet`].
pub struct Serving {
    port: u16,
    router: Box<dyn Fn(Subnet) -> Router + Send>,
    tasks: HashMap<Subnet, JoinHandle<()>>,
    /// Could not be bound at the last look: said once, tried again at each.
    failed: HashSet<Subnet>,
}

impl Serving {
    pub fn new(port: u16, router: Box<dyn Fn(Subnet) -> Router + Send>) -> Self {
        Serving {
            port,
            router,
            tasks: HashMap::new(),
            failed: HashSet::new(),
        }
    }

    pub async fn start(&mut self, subnet: Subnet) -> std::io::Result<()> {
        let listener = tokio::net::TcpListener::bind((subnet.addr, self.port)).await?;
        tracing::info!(
            "listening on http://{}/ (peers from {}/{})",
            listener
                .local_addr()
                .map_or(subnet.addr.to_string(), |a| a.to_string()),
            subnet.addr,
            subnet.prefix_len()
        );
        let app = (self.router)(subnet).into_make_service_with_connect_info::<SocketAddr>();
        let task = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("serve {}: {e}", subnet.addr);
            }
        });
        self.tasks.insert(subnet, task);
        Ok(())
    }

    /// Serves exactly `wanted`: listeners no longer wanted stop, new ones start. One that cannot be bound now
    /// is tried again at the next call.
    pub async fn follow(&mut self, wanted: &[Subnet]) {
        self.tasks.retain(|s, task| {
            let keep = wanted.contains(s);
            if !keep {
                task.abort();
                tracing::info!("stopped listening on {}", s.addr);
            }
            keep
        });
        self.failed.retain(|s| wanted.contains(s));
        for &s in wanted {
            if self.tasks.contains_key(&s) {
                continue;
            }
            match self.start(s).await {
                Ok(()) => {
                    self.failed.remove(&s);
                }
                Err(e) => {
                    if self.failed.insert(s) {
                        tracing::warn!("cannot listen on {}:{}: {e}", s.addr, self.port);
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct Guard {
    subnet: Subnet,
    allowed_origins: Arc<Vec<String>>,
}

/// The routes for one listener, whose peers must come from `subnet`.
pub fn router(ctx: Ctx, subnet: Subnet) -> Router {
    let guard = Guard {
        subnet,
        allowed_origins: ctx.allowed_origins.clone(),
    };
    Router::new()
        .route("/health", get(health))
        .route("/v1/rpc", post(rpc))
        .route("/v1/events", get(events))
        .nest_service("/web", web(&ctx.web_dir))
        .with_state(ctx)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(guard, check))
}

/// The static files under `/web/`, with the page's security headers.
fn web(dir: &std::path::Path) -> Router {
    Router::new()
        .fallback_service(ServeDir::new(dir))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(WEB_CSP),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
}

fn forbidden(why: &'static str) -> Response {
    (StatusCode::FORBIDDEN, why).into_response()
}

async fn check(
    State(g): State<Guard>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    if !peer_allowed(peer.ip(), &g.subnet) {
        return forbidden("来源地址不在本网段");
    }
    // A header that is not text matches nothing.
    let (Ok(origin), Ok(host)) = (
        header_text(req.headers(), header::ORIGIN),
        header_text(req.headers(), header::HOST),
    ) else {
        return forbidden("不接受这个来源");
    };
    if !host_allowed(host.as_deref(), &g.subnet) {
        return forbidden("不接受这个主机名");
    }
    match check_origin(origin.as_deref(), host.as_deref(), &g.allowed_origins) {
        OriginCheck::Deny => forbidden("不接受这个来源"),
        OriginCheck::Allow => next.run(req).await,
        OriginCheck::Cors => {
            let Some(origin) = origin.and_then(|o| HeaderValue::from_str(&o).ok()) else {
                return forbidden("不接受这个来源");
            };
            let preflight = req.method() == Method::OPTIONS;
            let mut res = if preflight {
                StatusCode::NO_CONTENT.into_response()
            } else {
                next.run(req).await
            };
            cors(res.headers_mut(), origin);
            res
        }
    }
}

fn header_text(h: &HeaderMap, name: header::HeaderName) -> Result<Option<String>, ()> {
    h.get(name)
        .map(|v| v.to_str().map(str::to_owned).map_err(|_| ()))
        .transpose()
}

fn cors(h: &mut HeaderMap, origin: HeaderValue) {
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    h.append(header::VARY, HeaderValue::from_static("Origin"));
}

async fn health() -> Json<Value> {
    Json(json!({
        "product": "CoDeck",
        "apiVersion": API_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(serde::Deserialize)]
struct RpcRequest {
    method: String,
    #[serde(default)]
    args: Value,
}

/// `Content-Type: application/json` (parameters aside): a page of another site cannot send that without a
/// CORS preflight, which it does not pass.
fn is_json(h: &HeaderMap) -> bool {
    h.get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
}

/// `{method, args}` → `{result}` or `{error: {code, message}}`; only `remote` methods.
async fn rpc(State(ctx): State<Ctx>, headers: HeaderMap, body: Bytes) -> Response {
    if !is_json(&headers) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "请求须是 application/json",
        )
            .into_response();
    }
    let req: RpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            let e = AppError {
                code: ErrorCode::InvalidQuery,
                message: format!("请求不是有效的 JSON：{e}"),
            };
            return Json(json!({ "error": e })).into_response();
        }
    };
    Json(
        match dispatch(&ctx.host, &req.method, req.args, true).await {
            Ok(result) => json!({ "result": result }),
            Err(e) => json!({ "error": e }),
        },
    )
    .into_response()
}

/// First `hello`, then every event as it happens; no history.
async fn events(ws: WebSocketUpgrade, State(ctx): State<Ctx>) -> Response {
    let rx = ctx.events.subscribe();
    ws.on_upgrade(move |socket| relay(socket, rx))
}

async fn relay(mut socket: WebSocket, mut rx: broadcast::Receiver<Utf8Bytes>) {
    let hello = json!({ "event": "hello", "payload": { "apiVersion": API_VERSION } }).to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(text) => {
                    if socket.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Events were missed: the client reconnects and resyncs.
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: 1013,
                            reason: "lagged".into(),
                        })))
                        .await;
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use codeck_host::HostConfig;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::fixtures;

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

    /// One HTTP/1.1 request with `Host: <addr>`; the status, the head and the body.
    async fn http(addr: SocketAddr, head: &str, body: &str) -> (u16, String, String) {
        http_as(addr, Some(&addr.to_string()), head, body).await
    }

    /// One HTTP/1.1 request with this `Host` header (none: without one).
    async fn http_as(
        addr: SocketAddr,
        host: Option<&str>,
        head: &str,
        body: &str,
    ) -> (u16, String, String) {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let host = host.map(|h| format!("\r\nHost: {h}")).unwrap_or_default();
        let req = format!(
            "{head}{host}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        let status = head[9..12].parse().unwrap();
        (status, head.to_ascii_lowercase(), body.to_string())
    }

    async fn rpc_call(addr: SocketAddr, body: Value) -> Value {
        let (status, _, body) = http(
            addr,
            "POST /v1/rpc HTTP/1.1\r\nContent-Type: application/json",
            &body.to_string(),
        )
        .await;
        assert_eq!(status, 200);
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn events_carry_connect_state_but_not_raw_ccrpc_lines() {
        let (tx, sink) = event_channel();
        let mut rx = tx.subscribe();
        sink(HostEvent::CcrpcLine {
            conn: "c1".into(),
            line: "{}".into(),
        });
        sink(HostEvent::CcrpcExit {
            conn: "c1".into(),
            code: None,
            stderr: String::new(),
        });
        sink(HostEvent::ConnectState(Box::new(
            codeck_host::connect::ConnState::new("c1", Some("H1".into())),
        )));
        sink(HostEvent::ConnectRemoved { conn: "c1".into() });
        sink(HostEvent::CcrpcLs { ls: "[]".into() });
        let got: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|t| serde_json::from_str(t.as_str()).unwrap())
            .collect();
        let names: Vec<_> = got.iter().map(|v| v["event"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "connect-state-event",
                "connect-removed-event",
                "ccrpc-ls-event"
            ]
        );
        assert_eq!(got[0]["payload"]["holderId"], "H1");
        assert_eq!(got[0]["payload"]["state"], "starting");
        assert_eq!(got[1]["payload"], json!({ "conn": "c1" }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn follow_starts_and_stops_listeners_as_addresses_come_and_go() {
        let free = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = free.local_addr().unwrap();
        drop(free);
        let subnet = Subnet {
            addr: Ipv4Addr::LOCALHOST,
            mask: Ipv4Addr::new(255, 0, 0, 0),
        };
        let mut serving = Serving::new(addr.port(), Box::new(|_| Router::new()));
        let answers = || async { tokio::net::TcpStream::connect(addr).await.is_ok() };
        // Stopping and starting are not instant: the old socket goes when its task does.
        async fn eventually(mut check: impl AsyncFnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !check().await {
                assert!(Instant::now() < deadline, "timed out");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        serving.follow(&[subnet]).await;
        assert!(answers().await);
        // Unchanged: the running listener stays.
        serving.follow(&[subnet]).await;
        assert_eq!(serving.tasks.len(), 1);
        assert!(serving.failed.is_empty());
        assert!(answers().await);

        serving.follow(&[]).await;
        assert!(serving.tasks.is_empty());
        eventually(async || !answers().await).await;

        // A new address comes up.
        eventually(async || {
            serving.follow(&[subnet]).await;
            serving.tasks.len() == 1
        })
        .await;
        assert!(answers().await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn serves_health_rpc_and_events_on_loopback() {
        let root = tempfile::tempdir().unwrap();
        copy_dir(&fixtures::fixture_dir("basic"), root.path());
        let state = tempfile::tempdir().unwrap();
        let web = tempfile::tempdir().unwrap();
        std::fs::write(web.path().join("transcript.html"), "<!doctype html>").unwrap();
        let (events, sink) = event_channel();
        let host = Host::start(
            HostConfig {
                cache_dir: state.path().join("cache"),
                settings_path: state.path().join("settings.json"),
                ccrpc_logs: Some(state.path().join("logs/ccrpc")),
                images: Some(state.path().join("images")),
                data_root: Some(root.path().to_path_buf()),
                refresh_prices: false,
                ..HostConfig::desktop()
            },
            sink,
        )
        .unwrap();
        let ctx = Ctx {
            host,
            events,
            allowed_origins: Arc::new(vec!["http://localhost:1420".into()]),
            web_dir: web.path().to_path_buf(),
        };
        let subnet = Subnet {
            addr: Ipv4Addr::LOCALHOST,
            mask: Ipv4Addr::new(255, 0, 0, 0),
        };
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(ctx, subnet).into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move { axum::serve(listener, app).await });

        let (status, _, body) = http(addr, "GET /health HTTP/1.1", "").await;
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["product"], "CoDeck");
        assert_eq!(v["apiVersion"], 1);

        // The first scan runs in the background.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let v = rpc_call(addr, json!({ "method": "list_projects" })).await;
            let n = v["result"].as_array().expect("result").len();
            if n > 0 {
                break;
            }
            assert!(Instant::now() < deadline, "no Project indexed");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let v = rpc_call(addr, json!({ "method": "rebuild_index", "args": {} })).await;
        assert_eq!(v["error"]["code"], "invalidQuery", "{v}");
        // Without `track_connect` there is no Connect state to serve.
        let v = rpc_call(addr, json!({ "method": "connect_states" })).await;
        assert_eq!(v["error"]["code"], "invalidQuery", "{v}");
        let v = rpc_call(addr, json!({ "method": "no_such_method" })).await;
        assert_eq!(v["error"]["code"], "notFound", "{v}");

        let (status, head, body) = http(addr, "GET /web/transcript.html HTTP/1.1", "").await;
        assert_eq!((status, body.as_str()), (200, "<!doctype html>"));
        assert!(head.contains("content-type: text/html"), "{head}");
        assert!(head.contains("x-content-type-options: nosniff"), "{head}");
        assert!(
            head.contains("content-security-policy: default-src 'self';"),
            "{head}"
        );
        let (_, head, _) = http(addr, "GET /health HTTP/1.1", "").await;
        assert!(!head.contains("content-security-policy"), "{head}");

        // A name other than the listener's (DNS rebinding), even with a matching Origin, or no Host at all.
        let evil = format!("rebind.attacker.example:{}", addr.port());
        let rpc_head = "POST /v1/rpc HTTP/1.1\r\nContent-Type: application/json";
        let body = json!({ "method": "list_projects" }).to_string();
        let (status, _, why) = http_as(
            addr,
            Some(&evil),
            &format!("{rpc_head}\r\nOrigin: http://{evil}"),
            &body,
        )
        .await;
        assert_eq!((status, why.as_str()), (403, "不接受这个主机名"));
        let (status, ..) = http_as(addr, None, rpc_head, &body).await;
        assert_eq!(status, 403);
        let upgrade = "GET /v1/events HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13";
        let (status, ..) = http_as(addr, Some(&evil), upgrade, "").await;
        assert_eq!(status, 403);
        // `localhost` names a loopback listener.
        let (status, ..) = http_as(
            addr,
            Some(&format!("localhost:{}", addr.port())),
            "GET /health HTTP/1.1",
            "",
        )
        .await;
        assert_eq!(status, 200);

        // Only JSON bodies: a form post needs no CORS preflight.
        for ct in ["text/plain", "application/x-www-form-urlencoded"] {
            let (status, ..) = http(
                addr,
                &format!("POST /v1/rpc HTTP/1.1\r\nContent-Type: {ct}"),
                &body,
            )
            .await;
            assert_eq!(status, 415, "{ct}");
        }
        let (status, ..) = http(addr, "POST /v1/rpc HTTP/1.1", &body).await;
        assert_eq!(status, 415);
        let (status, ..) = http(
            addr,
            "POST /v1/rpc HTTP/1.1\r\nContent-Type: Application/JSON; charset=utf-8",
            &body,
        )
        .await;
        assert_eq!(status, 200);
        let (status, ..) = http(addr, "GET /web/../Cargo.toml HTTP/1.1", "").await;
        assert_eq!(status, 404);

        // Browser origins: the allowed one gets CORS, `null` and others are refused.
        let (status, head, _) = http(
            addr,
            "OPTIONS /v1/rpc HTTP/1.1\r\nOrigin: http://localhost:1420\r\nAccess-Control-Request-Method: POST",
            "",
        )
        .await;
        assert_eq!(status, 204);
        assert!(head.contains("access-control-allow-origin: http://localhost:1420"));
        for origin in ["null", "http://evil.example"] {
            let (status, ..) = http(
                addr,
                &format!("GET /health HTTP/1.1\r\nOrigin: {origin}"),
                "",
            )
            .await;
            assert_eq!(status, 403, "{origin}");
        }
        let (status, ..) = http(
            addr,
            &format!("GET /health HTTP/1.1\r\nOrigin: http://{addr}"),
            "",
        )
        .await;
        assert_eq!(status, 200);

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/events"))
            .await
            .unwrap();
        use futures_util::StreamExt;
        let first = ws.next().await.unwrap().unwrap();
        let v: Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
        assert_eq!(
            v,
            json!({ "event": "hello", "payload": { "apiVersion": 1 } })
        );
    }
}
