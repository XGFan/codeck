//! The HTTP + WebSocket face of the Host: `/health`, `POST /v1/rpc`, `GET /v1/events`, `/web/*`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::serve::ListenerExt;
use axum::{Json, Router};
use codeck_core::model::{AppError, ErrorCode};
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

use super::devices::DeviceRevoke;
use super::net::{OriginCheck, Subnet, check_origin, host_allowed, peer_allowed};
use super::security::{LanSecurity, RedeemError};
use super::tls::TlsListener;
use crate::{EventSink, Host, HostEvent};

/// Bumped only on incompatible changes to the methods, events or envelopes.
pub const API_VERSION: u32 = 2;
const BODY_LIMIT: usize = 32 * 1024 * 1024;
const PAIR_BODY_LIMIT: usize = 4 * 1024;
/// Overall timeout for `POST /v1/pair` so slow request bodies cannot hold TLS connections open.
const PAIR_TIMEOUT: Duration = Duration::from_secs(10);
/// Events a slow client may fall behind by before it is dropped (it resyncs on reconnect).
const EVENT_BACKLOG: usize = 1024;
/// The Transcript page's policy: only this Host's scripts, styles, images and connections (the Shiki
/// worker is a blob), never framed.
const WEB_CSP: &str = "default-src 'self'; script-src 'self'; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

#[derive(Clone)]
pub struct Ctx {
    pub host: Arc<Host>,
    pub events: broadcast::Sender<Utf8Bytes>,
    pub allowed_origins: Arc<Vec<String>>,
    pub web_dir: PathBuf,
    pub gate: Gate,
    /// `Some` on non-loopback TLS listeners (enforces bearer auth and exposes `/v1/pair`); `None` on loopback.
    pub security: Option<Arc<LanSecurity>>,
}

impl Ctx {
    pub fn with_security(&self, security: Option<Arc<LanSecurity>>) -> Self {
        let mut c = self.clone();
        c.security = security;
        c
    }
}

/// Whether what is already connected is still served. A stopped listener's upgraded WebSockets and the requests
/// reaching it meanwhile are tasks of their own and would go on, so turning LAN access off closes this too. The
/// daemon's never closes.
#[derive(Clone, Default)]
pub struct Gate(watch::Sender<bool>);

impl Gate {
    pub fn close(&self) {
        // Not `send`, which fails while no one is subscribed.
        self.0.send_replace(true);
    }

    pub fn is_closed(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once closed, at once if already.
    pub async fn closed(&self) {
        let mut rx = self.0.subscribe();
        // Fails only without a sender, and `self` is one.
        let _ = rx.wait_for(|closed| *closed).await;
    }
}

/// The broadcast channel the WebSocket clients read, and the Host's sink writing to it: every event as
/// `{event, payload}`, except ccrpc's raw lines and exits (remote clients get the Connect state instead:
/// `connect-state-event`, `connect-removed-event`) and what only tells the desktop's front end about a remote
/// client's connections.
pub fn event_channel() -> (broadcast::Sender<Utf8Bytes>, EventSink) {
    let (tx, _) = broadcast::channel(EVENT_BACKLOG);
    let sink_tx = tx.clone();
    let sink: EventSink = Arc::new(move |e: HostEvent| {
        // On the desktop app most of the time no one is listening: not worth serializing every event for.
        if sink_tx.receiver_count() == 0
            || matches!(
                e,
                HostEvent::CcrpcLine { .. }
                    | HostEvent::CcrpcExit { .. }
                    | HostEvent::CcrpcRemoteConn { .. }
                    | HostEvent::CcrpcClosing { .. }
                    | HostEvent::HaaWorkers { .. }
                    | HostEvent::HaaWatchEnded { .. }
                    | HostEvent::KeepAlive { .. }
                    | HostEvent::LanDevices { .. }
            )
        {
            return;
        }
        let msg = json!({ "event": e.name(), "payload": e.payload() }).to_string();
        let _ = sink_tx.send(msg.into());
    });
    (tx, sink)
}

/// Whether a listener on `subnet` must speak TLS and require pairing auth (every address except loopback).
pub fn needs_tls(subnet: &Subnet) -> bool {
    !subnet.addr.is_loopback()
}

type RouterFactory = Box<dyn Fn(Subnet, Option<Arc<LanSecurity>>) -> Router + Send>;

#[cfg(test)]
type BindOverride = Box<dyn Fn(Subnet) -> std::io::Result<std::net::TcpListener> + Send>;

/// The listeners being served, one per [`Subnet`].
pub struct Serving {
    port: u16,
    router: RouterFactory,
    security: Option<Arc<LanSecurity>>,
    tasks: HashMap<Subnet, JoinHandle<()>>,
    /// Active listeners in interface/wanted order (never numerically sorted).
    order: Vec<Subnet>,
    /// Could not be bound at the last look, and why: logged once, tried again at each.
    failed: HashMap<Subnet, String>,
    /// Live mirror of `self.listening_in_order()` updated on every `start` / `follow` / `stop`.
    snapshot: Arc<parking_lot::Mutex<Vec<String>>>,
    #[cfg(test)]
    bind_override: Option<BindOverride>,
}

impl Serving {
    pub fn new(port: u16, router: RouterFactory, security: Option<Arc<LanSecurity>>) -> Self {
        Serving {
            port,
            router,
            security,
            tasks: HashMap::new(),
            order: Vec::new(),
            failed: HashMap::new(),
            snapshot: Arc::default(),
            #[cfg(test)]
            bind_override: None,
        }
    }

    /// Shared live view of the `address:port`s currently bound by this [`Serving`], in listening
    /// (interface enumeration) order for pairing links.
    pub fn listening_handle(&self) -> Arc<parking_lot::Mutex<Vec<String>>> {
        Arc::clone(&self.snapshot)
    }

    fn sync_snapshot(&self) {
        *self.snapshot.lock() = self.listening_in_order();
    }

    pub async fn start(&mut self, subnet: Subnet) -> std::io::Result<()> {
        let (tls, route_security) = if needs_tls(&subnet) {
            let Some(sec) = self.security.clone() else {
                return Err(std::io::Error::other(format!(
                    "没有 TLS 与鉴权配置，不在 {} 上监听",
                    subnet.addr
                )));
            };
            let sec_for_cfg = Arc::clone(&sec);
            let cfg = tokio::task::spawn_blocking(move || sec_for_cfg.server_config())
                .await
                .map_err(std::io::Error::other)?
                .map_err(std::io::Error::other)?;
            (Some(cfg), Some(sec))
        } else {
            (None, None)
        };
        #[cfg(test)]
        let listener = if let Some(binder) = &self.bind_override {
            let std_listener = binder(subnet)?;
            std_listener.set_nonblocking(true)?;
            tokio::net::TcpListener::from_std(std_listener)?
        } else {
            tokio::net::TcpListener::bind((subnet.addr, self.port)).await?
        };
        #[cfg(not(test))]
        let listener = tokio::net::TcpListener::bind((subnet.addr, self.port)).await?;
        let scheme = if tls.is_some() { "https" } else { "http" };
        tracing::info!(
            "listening on {scheme}://{}/ (peers from {}/{})",
            listener
                .local_addr()
                .map_or(subnet.addr.to_string(), |a| a.to_string()),
            subnet.addr,
            subnet.prefix_len()
        );
        let app = (self.router)(subnet, route_security)
            .into_make_service_with_connect_info::<SocketAddr>();
        let task = tokio::spawn(async move {
            let res = match tls {
                Some(cfg) => {
                    // `tap_io` supplies `Connected<IncomingStream<TapIo<L, _>>> for SocketAddr` in axum 0.8.
                    let listener = TlsListener::new(listener, cfg).tap_io(|_| ());
                    axum::serve(listener, app).await
                }
                None => axum::serve(listener, app).await,
            };
            if let Err(e) = res {
                tracing::error!("serve {}: {e}", subnet.addr);
            }
        });
        self.tasks.insert(subnet, task);
        if !self.order.contains(&subnet) {
            self.order.push(subnet);
        }
        self.sync_snapshot();
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
        self.failed.retain(|s, _| wanted.contains(s));
        for &s in wanted {
            if self.tasks.contains_key(&s) {
                continue;
            }
            match self.start(s).await {
                Ok(()) => {
                    self.failed.remove(&s);
                }
                Err(e) => {
                    let why = if needs_tls(&s) && self.security.is_none() {
                        e.to_string()
                    } else if e.kind() == std::io::ErrorKind::AddrInUse {
                        format!("{}:{} 已被占用", s.addr, self.port)
                    } else {
                        format!("监听不了 {}:{}：{e}", s.addr, self.port)
                    };
                    if self.failed.insert(s, why).is_none() {
                        tracing::warn!("cannot listen on {}:{}: {e}", s.addr, self.port);
                    }
                }
            }
        }
        self.order.clear();
        for &s in wanted {
            if self.tasks.contains_key(&s) && !self.order.contains(&s) {
                self.order.push(s);
            }
        }
        self.sync_snapshot();
    }

    /// Stops every listener; their port is free when this returns.
    pub async fn stop(&mut self) {
        let tasks: Vec<_> = self.tasks.drain().map(|(_, task)| task).collect();
        self.order.clear();
        self.sync_snapshot();
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }

    /// The `address:port`s being listened on, sorted for stable settings UI display.
    pub fn listening(&self) -> Vec<String> {
        let mut addrs: Vec<_> = self.tasks.keys().map(|s| s.addr).collect();
        addrs.sort();
        addrs
            .into_iter()
            .map(|a| format!("{a}:{}", self.port))
            .collect()
    }

    /// The `address:port`s being listened on, preserving interface/listening order for pairing offers.
    pub fn listening_in_order(&self) -> Vec<String> {
        self.order
            .iter()
            .map(|s| format!("{}:{}", s.addr, self.port))
            .collect()
    }

    /// Why each address that should be listened on is not.
    pub fn errors(&self) -> Vec<String> {
        let mut why: Vec<_> = self.failed.iter().collect();
        why.sort_by_key(|(s, _)| s.addr);
        why.into_iter().map(|(_, e)| e.clone()).collect()
    }
}

#[derive(Clone)]
struct Guard {
    subnet: Subnet,
    allowed_origins: Arc<Vec<String>>,
    gate: Gate,
    security: Option<Arc<LanSecurity>>,
}

/// Authenticated device attached to request extensions by [`check`].
#[derive(Clone)]
struct AuthedDevice {
    id: String,
    revoke: DeviceRevoke,
}

/// The routes for one listener, whose peers must come from `subnet`.
pub fn router(ctx: Ctx, subnet: Subnet) -> Router {
    router_with_pair_timeout(ctx, subnet, PAIR_TIMEOUT)
}

/// Same as [`router`], with a custom overall timeout for `POST /v1/pair`.
pub fn router_with_pair_timeout(ctx: Ctx, subnet: Subnet, pair_timeout: Duration) -> Router {
    let guard = Guard {
        subnet,
        allowed_origins: ctx.allowed_origins.clone(),
        gate: ctx.gate.clone(),
        security: ctx.security.clone(),
    };
    let mut r = Router::new()
        .route("/health", get(health))
        .merge(
            Router::new()
                .route("/v1/rpc", post(rpc))
                .layer(DefaultBodyLimit::max(BODY_LIMIT)),
        )
        .route("/v1/events", get(events))
        .nest_service("/web", web(&ctx.web_dir));
    if ctx.security.is_some() {
        r = r.merge(
            Router::new()
                .route("/v1/pair", post(pair))
                .layer(DefaultBodyLimit::max(PAIR_BODY_LIMIT))
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    pair_timeout,
                )),
        );
    }
    r.with_state(ctx)
        .layer(middleware::from_fn_with_state(guard, check))
        .layer(middleware::from_fn(no_store_sensitive))
}

/// Ensures every response for `/v1/rpc` and `/v1/pair` (including early auth/origin/limit/timeout
/// rejections) carries `Cache-Control: no-store`, and closes connections on `408 Request Timeout`.
async fn no_store_sensitive(req: Request, next: Next) -> Response {
    let sensitive = matches!(req.uri().path(), "/v1/rpc" | "/v1/pair");
    let mut res = next.run(req).await;
    if res.status() == StatusCode::REQUEST_TIMEOUT {
        res.headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
    if sensitive {
        res.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
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

fn unauthorized(close: bool) -> Response {
    let body = Json(json!({
        "error": {
            "code": "unauthorized",
            "message": "这台设备没有配对或已被移除，请重新扫码配对。",
        }
    }));
    if close {
        (
            StatusCode::UNAUTHORIZED,
            [(header::CONNECTION, "close")],
            body,
        )
            .into_response()
    } else {
        (StatusCode::UNAUTHORIZED, body).into_response()
    }
}

/// Only exact `(method, path)` pairs exempt from bearer auth on non-loopback listeners.
fn is_public_route(method: &Method, path: &str) -> bool {
    matches!(
        (method, path),
        (&Method::GET | &Method::HEAD, "/health") | (&Method::POST, "/v1/pair")
    )
}

fn bearer_token(h: &HeaderMap) -> Option<&str> {
    let v = h.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

async fn check(
    State(g): State<Guard>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut req: Request,
    next: Next,
) -> Response {
    if g.gate.is_closed() {
        return gate_closed();
    }
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
    let cors_origin = match check_origin(
        origin.as_deref(),
        host.as_deref(),
        &g.allowed_origins,
        g.security.is_some(),
    ) {
        OriginCheck::Deny => return forbidden("不接受这个来源"),
        OriginCheck::Allow => None,
        OriginCheck::Cors => {
            let Some(origin) = origin.and_then(|o| HeaderValue::from_str(&o).ok()) else {
                return forbidden("不接受这个来源");
            };
            if req.method() == Method::OPTIONS {
                let mut res = StatusCode::NO_CONTENT.into_response();
                cors(res.headers_mut(), origin);
                return res;
            }
            Some(origin)
        }
    };

    let mut revoke = None;
    if let Some(security) = &g.security
        && !is_public_route(req.method(), req.uri().path())
    {
        let Some((id, sig)) = bearer_token(req.headers()).and_then(|t| security.verify(t)) else {
            let mut res = unauthorized(false);
            if let Some(origin) = cors_origin {
                cors(res.headers_mut(), origin);
            }
            return res;
        };
        req.extensions_mut().insert(AuthedDevice {
            id,
            revoke: sig.clone(),
        });
        revoke = Some(sig);
    }

    let mut res = gated(&g.gate, revoke.as_ref(), next, req).await;
    if let Some(origin) = cors_origin {
        cors(res.headers_mut(), origin);
    }
    res
}

/// The connection goes too: this Gate stays closed, even once LAN access is on again with a new one.
fn gate_closed() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONNECTION, "close")],
        "局域网访问已关闭",
    )
        .into_response()
}

/// The request, unless the Gate closes or the device is revoked first: one past the check may still be
/// sending its body, and would otherwise run whenever that arrives.
async fn gated(gate: &Gate, revoke: Option<&DeviceRevoke>, next: Next, req: Request) -> Response {
    tokio::select! {
        biased;
        _ = gate.closed() => gate_closed(),
        _ = async {
            match revoke {
                Some(r) => r.revoked().await,
                None => std::future::pending().await,
            }
        } => unauthorized(true),
        res = next.run(req) => {
            // Re-check before handing the response out in case revocation or gate close happened
            // synchronously while the handler was finishing.
            if gate.is_closed() {
                gate_closed()
            } else if revoke.is_some_and(DeviceRevoke::is_revoked) {
                unauthorized(true)
            } else {
                res
            }
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
async fn rpc(
    State(ctx): State<Ctx>,
    authed: Option<Extension<AuthedDevice>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
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
    // Re-check after reading the body and right before dispatching side effects.
    if let Some(Extension(a)) = authed
        && a.revoke.is_revoked()
    {
        return unauthorized(true);
    }
    Json(match ctx.host.remote_call(&req.method, req.args).await {
        Ok(result) => json!({ "result": result }),
        Err(e) => json!({ "error": e }),
    })
    .into_response()
}

#[derive(serde::Deserialize)]
struct PairRequest {
    key: String,
    #[serde(default)]
    name: String,
}

/// Exchanges a single-use pairing key for a device id and bearer token (only mounted on non-loopback listeners).
async fn pair(State(ctx): State<Ctx>, headers: HeaderMap, body: Bytes) -> Response {
    if !is_json(&headers) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "请求须是 application/json",
        )
            .into_response();
    }
    let Some(security) = &ctx.security else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let req: PairRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "code": "invalid_request",
                        "message": format!("请求不是有效的 JSON：{e}"),
                    }
                })),
            )
                .into_response();
        }
    };
    let sec = Arc::clone(security);
    let key = req.key;
    let name = req.name;
    let redeemed = match tokio::task::spawn_blocking(move || sec.redeem(&key, &name)).await {
        Ok(r) => r,
        Err(e) => Err(RedeemError::SaveFailed(e.to_string())),
    };
    match redeemed {
        Ok(res) => {
            ctx.host.emit_event(HostEvent::LanDevices {
                devices: res.devices,
                paired_offer: Some(res.offer_id),
                paired_device: Some(res.device.id.clone()),
            });
            Json(json!({
                "result": {
                    "deviceId": res.device.id,
                    "token": res.token,
                }
            }))
            .into_response()
        }
        Err(RedeemError::Expired) => (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": {
                    "code": "pairing_expired",
                    "message": "配对码已过期或已用过，请在 Host 上重新生成。",
                }
            })),
        )
            .into_response(),
        Err(RedeemError::SaveFailed(why)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": {
                    "code": "pairing_failed",
                    "message": format!("保存配对失败：{why}"),
                }
            })),
        )
            .into_response(),
    }
}

/// First `hello`, then every event as it happens; no history.
async fn events(
    ws: WebSocketUpgrade,
    State(ctx): State<Ctx>,
    authed: Option<Extension<AuthedDevice>>,
) -> Response {
    let authed = authed.map(|Extension(a)| a);
    if let Some(ref a) = authed
        && a.revoke.is_revoked()
    {
        return unauthorized(true);
    }
    let rx = ctx.events.subscribe();
    ws.on_upgrade(move |socket| run_ws_session(socket, ctx, rx, authed))
}

async fn run_ws_session(
    socket: WebSocket,
    ctx: Ctx,
    rx: broadcast::Receiver<Utf8Bytes>,
    authed: Option<AuthedDevice>,
) {
    let revoke = if let Some(a) = authed {
        // Re-check after upgrade completes in case the device was removed in the gap.
        if a.revoke.is_revoked() {
            drop(socket);
            return;
        }
        if let Some(sec) = &ctx.security {
            let sec = Arc::clone(sec);
            let id = a.id.clone();
            if let Ok(Some(devices)) = tokio::task::spawn_blocking(move || sec.touch(&id)).await {
                ctx.host.emit_event(HostEvent::LanDevices {
                    devices,
                    paired_offer: None,
                    paired_device: None,
                });
            }
        }
        if a.revoke.is_revoked() {
            drop(socket);
            return;
        }
        Some(a.revoke)
    } else {
        None
    };
    tokio::select! {
        biased;
        _ = async {
            match revoke {
                Some(r) => r.revoked().await,
                None => std::future::pending().await,
            }
        } => {}
        _ = relay(socket, rx, ctx.gate) => {}
    }
}

async fn relay(mut socket: WebSocket, mut rx: broadcast::Receiver<Utf8Bytes>, gate: Gate) {
    let hello = json!({ "event": "hello", "payload": { "apiVersion": API_VERSION } }).to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            biased;
            _ = gate.closed() => {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: 1001,
                        reason: "closed".into(),
                    })))
                    .await;
                break;
            }
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

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::{HostConfig, fixtures};

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
            crate::connect::ConnState::new("c1", Some("H1".into())),
        )));
        sink(HostEvent::ConnectRemoved { conn: "c1".into() });
        sink(HostEvent::CcrpcRemoteConn {
            conn: "c1".into(),
            holder_id: Some("H1".into()),
        });
        sink(HostEvent::CcrpcClosing { conn: "c1".into() });
        sink(HostEvent::HaaWorkers {
            id: 1,
            workers: "[]".into(),
        });
        sink(HostEvent::HaaWatchEnded {
            id: 1,
            message: "ended".into(),
        });
        sink(HostEvent::LanDevices {
            devices: vec![],
            paired_offer: None,
            paired_device: None,
        });
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
        let mut serving = Serving::new(addr.port(), Box::new(|_, _| Router::new()), None);
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
        let gate = Gate::default();
        let ctx = Ctx {
            host,
            events,
            allowed_origins: Arc::new(vec!["http://localhost:1420".into()]),
            web_dir: web.path().to_path_buf(),
            gate: gate.clone(),
            security: None,
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
        assert_eq!(v["apiVersion"], 2);

        // Loopback exposes no `/v1/pair`.
        let (status, ..) = http(
            addr,
            "POST /v1/pair HTTP/1.1\r\nContent-Type: application/json",
            r#"{"key":"k","name":"n"}"#,
        )
        .await;
        assert_eq!(status, 404);

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
            json!({ "event": "hello", "payload": { "apiVersion": 2 } })
        );

        // LAN access goes off with the listener still up: the WebSocket is closed, a request refused, and that
        // request's connection ended by the server (the read below returns only then).
        gate.close();
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        assert!(
            matches!(ws.next().await.unwrap().unwrap(), WsMessage::Close(Some(f)) if u16::from(f.code) == 1001)
        );
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(format!("GET /health HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.starts_with("HTTP/1.1 403"), "{out}");
        assert!(out.ends_with("局域网访问已关闭"), "{out}");
    }

    #[tokio::test]
    async fn non_loopback_without_security_is_refused() {
        let lan = Subnet {
            addr: Ipv4Addr::new(192, 168, 1, 20),
            mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let lo = Subnet {
            addr: Ipv4Addr::LOCALHOST,
            mask: Ipv4Addr::new(255, 0, 0, 0),
        };
        assert!(needs_tls(&lan));
        assert!(!needs_tls(&lo));

        let mut serving = Serving::new(0, Box::new(|_, _| Router::new()), None);
        let err = serving.start(lan).await.unwrap_err();
        assert!(err.to_string().contains("没有 TLS 与鉴权配置"), "{err}");

        serving.follow(&[lan]).await;
        assert!(serving.listening().is_empty());
        assert_eq!(
            serving.errors(),
            ["没有 TLS 与鉴权配置，不在 192.168.1.20 上监听"]
        );
    }

    /// Simulates revocation landing in the gap between auth middleware and `events` / `on_upgrade`.
    #[tokio::test(flavor = "multi_thread")]
    async fn ws_revoked_in_upgrade_gap_is_rejected_and_not_touched() {
        let dir = tempfile::tempdir().unwrap();
        let sec = Arc::new(LanSecurity::open(dir.path().to_path_buf()).unwrap());
        let offer = sec.start_pairing().unwrap();
        let paired = sec.redeem(&offer.key, "iPhone").unwrap();
        let (dev_id, revoke) = sec.verify(&paired.token).unwrap();

        let (events_tx, sink) = event_channel();
        let host = Host::start(
            HostConfig {
                cache_dir: dir.path().join("cache"),
                settings_path: dir.path().join("settings.json"),
                ccrpc_logs: Some(dir.path().join("logs")),
                images: Some(dir.path().join("images")),
                data_root: Some(dir.path().join("root")),
                refresh_prices: false,
                ..HostConfig::desktop()
            },
            sink,
        )
        .unwrap();
        let ctx = Ctx {
            host,
            events: events_tx,
            allowed_origins: Arc::default(),
            web_dir: dir.path().join("web"),
            gate: Gate::default(),
            security: Some(Arc::clone(&sec)),
        };

        // Pre-revoke the device before mounting a handler that runs `events` directly (simulating
        // revocation right after `check` inserted `AuthedDevice`) and one that runs `run_ws_session`
        // directly (simulating revocation right after 101 Switching Protocols).
        sec.remove(&dev_id).unwrap();
        assert!(revoke.is_revoked());
        let authed = AuthedDevice {
            id: dev_id.clone(),
            revoke: revoke.clone(),
        };

        let app = Router::new()
            .route(
                "/before-upgrade",
                get({
                    let authed = authed.clone();
                    move |ws: WebSocketUpgrade, state: State<Ctx>| {
                        events(ws, state, Some(Extension(authed.clone())))
                    }
                }),
            )
            .route(
                "/after-upgrade",
                get(move |ws: WebSocketUpgrade, State(ctx): State<Ctx>| {
                    let authed = authed.clone();
                    async move {
                        let rx = ctx.events.subscribe();
                        ws.on_upgrade(move |socket| run_ws_session(socket, ctx, rx, Some(authed)))
                    }
                }),
            )
            .with_state(ctx);

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });

        // Gap 1: revoked before `events` returns 101 -> receives 401.
        let err = tokio_tungstenite::connect_async(format!("ws://{addr}/before-upgrade"))
            .await
            .unwrap_err();
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            }
            other => panic!("expected HTTP 401, got {other:?}"),
        }

        // Gap 2: revoked after 101, inside `on_upgrade` -> closes without sending `hello`.
        let (mut ws, resp) = tokio_tungstenite::connect_async(format!("ws://{addr}/after-upgrade"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
        use futures_util::StreamExt;
        match tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("ws should close immediately")
        {
            None | Some(Err(_)) => {}
            Some(Ok(msg)) => panic!("revoked ws unexpectedly sent frame: {msg:?}"),
        }
    }

    /// Regression: when a handler revokes the device and becomes ready on the same poll (or yields
    /// so both `select!` arms are ready simultaneously), `gated` must still return 401 + `no-store`.
    #[tokio::test(flavor = "multi_thread")]
    async fn gated_prefers_revocation_when_handler_finishes_simultaneously() {
        let dir = tempfile::tempdir().unwrap();
        let sec = Arc::new(LanSecurity::open(dir.path().to_path_buf()).unwrap());

        let subnet = Subnet {
            addr: Ipv4Addr::LOCALHOST,
            mask: Ipv4Addr::new(255, 0, 0, 0),
        };
        let guard = Guard {
            subnet,
            allowed_origins: Arc::default(),
            gate: Gate::default(),
            security: Some(Arc::clone(&sec)),
        };

        let sec_sync = Arc::clone(&sec);
        let sec_yield = Arc::clone(&sec);
        let app = Router::new()
            // Handler revokes and returns 200 synchronously without awaiting -> hits post-`next.run` check.
            .route(
                "/v1/rpc",
                post(move |Extension(authed): Extension<AuthedDevice>| {
                    let sec = Arc::clone(&sec_sync);
                    async move {
                        sec.remove(&authed.id).unwrap();
                        (StatusCode::OK, "should-not-leak")
                    }
                }),
            )
            // Handler revokes then yields once so both `revoke.revoked()` and `next.run()` are ready
            // on the next poll -> hits `biased;` select priority.
            .route(
                "/yield-revoke",
                post(move |Extension(authed): Extension<AuthedDevice>| {
                    let sec = Arc::clone(&sec_yield);
                    async move {
                        sec.remove(&authed.id).unwrap();
                        tokio::task::yield_now().await;
                        (StatusCode::OK, "should-not-leak")
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(guard, check))
            .layer(middleware::from_fn(no_store_sensitive))
            .into_make_service_with_connect_info::<SocketAddr>();

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        for (path, expect_no_store) in [("/v1/rpc", true), ("/yield-revoke", false)] {
            let offer = sec.start_pairing().unwrap();
            let paired = sec.redeem(&offer.key, "iPhone").unwrap();
            let (status, head, body) = http(
                addr,
                &format!(
                    "POST {path} HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json",
                    paired.token
                ),
                "{}",
            )
            .await;
            assert_eq!(status, 401, "{path}: {body}");
            assert!(head.contains("connection: close"), "{path}: {head}");
            assert!(!body.contains("should-not-leak"), "{path}: {body}");
            if expect_no_store {
                assert!(head.contains("cache-control: no-store"), "{path}: {head}");
            }
        }

        srv.abort();
        let _ = srv.await;
    }

    /// Regression: pairing addresses keep `listeners()` / `wanted` order (not numerically sorted)
    /// and exclude addresses that failed to bind, while `listening()` stays sorted for settings UI.
    #[tokio::test(flavor = "multi_thread")]
    async fn serving_preserves_listening_order_and_excludes_failed_binds() {
        let dir = tempfile::tempdir().unwrap();
        let sec = Arc::new(LanSecurity::open(dir.path().to_path_buf()).unwrap());
        let first = Subnet {
            addr: Ipv4Addr::new(192, 168, 1, 20),
            mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let failing = Subnet {
            addr: Ipv4Addr::new(172, 16, 0, 5),
            mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let second = Subnet {
            addr: Ipv4Addr::new(10, 0, 0, 5),
            mask: Ipv4Addr::new(255, 255, 255, 0),
        };

        let port = 38471;
        let mut serving =
            Serving::new(port, Box::new(|_, _| Router::new()), Some(Arc::clone(&sec)));
        serving.bind_override = Some(Box::new(move |s| {
            if s == failing {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    "simulated bind failure",
                ))
            } else {
                std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            }
        }));
        let handle = serving.listening_handle();

        serving.follow(&[first, failing, second]).await;
        // UI display list stays numerically sorted.
        assert_eq!(
            serving.listening(),
            [format!("10.0.0.5:{port}"), format!("192.168.1.20:{port}")]
        );
        // Pairing snapshot preserves wanted order and excludes the failed bind.
        let ordered = serving.listening_in_order();
        assert_eq!(
            ordered,
            [format!("192.168.1.20:{port}"), format!("10.0.0.5:{port}")]
        );
        assert_eq!(*handle.lock(), ordered);
        assert_eq!(serving.errors().len(), 1);
        assert!(
            serving.errors()[0].contains("172.16.0.5"),
            "{:?}",
            serving.errors()
        );

        let offer = crate::lan::pairing::create_pairing_offer(&sec, port, &ordered).unwrap();
        assert!(
            offer
                .url
                .starts_with(&format!("codeck://pair?h=192.168.1.20,10.0.0.5&p={port}&")),
            "{}",
            offer.url
        );

        serving.stop().await;
        assert!(serving.listening_in_order().is_empty());
        assert!(handle.lock().is_empty());
    }
}
