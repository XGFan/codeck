//! Self-signed TLS certificate persistence and the axum [`TlsListener`] for non-loopback LAN listeners.

use std::collections::HashMap;
use std::future::Future;
use std::io::{self, IoSlice, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine;
use chrono::Datelike;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use parking_lot::Mutex;
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

const TLS_DIR: &str = "tls";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";
/// Certificate lifetime: ~10 years (including leap days), starting a day early for clock skew.
const CERT_VALID_DAYS: u64 = 3653;
/// Cap concurrent in-flight TLS handshakes so stalled peers cannot exhaust file descriptors.
const MAX_HANDSHAKES: usize = 64;
/// Cap total active TLS connections (handshaking + established) shared across all listeners in this process.
pub const MAX_CONNECTIONS: usize = 128;
/// Cap active TLS connections (handshaking + established) from one source IP across all listeners in this process.
pub const MAX_CONNECTIONS_PER_IP: usize = 16;
/// Close established TLS connections that receive no incoming bytes for this duration (iPhone WebSockets ping every 20s).
pub const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(75);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The Host's self-signed TLS certificate, private key, and SHA-256 fingerprint.
pub struct HostCert {
    pub cert_der: CertificateDer<'static>,
    pub key_der: PrivateKeyDer<'static>,
    /// Base64url (no padding, 43 chars) SHA-256 of `cert_der`.
    pub fingerprint: String,
    config: Arc<ServerConfig>,
}

impl std::fmt::Debug for HostCert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostCert")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// Base64url (no padding, 43 chars) SHA-256 of a DER-encoded certificate.
pub fn cert_fingerprint(cert_der: &[u8]) -> String {
    let digest = Sha256::digest(cert_der);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

impl HostCert {
    /// Loads `dir/tls/{cert.pem,key.pem}`, or creates a new ECDSA P-256 self-signed certificate if `dir/tls/`
    /// does not exist. If `dir/tls/` already exists but is incomplete, unreadable, or mismatched, returns an
    /// error instead of regenerating (which would invalidate pinned clients).
    pub fn load_or_create(dir: &Path) -> Result<Self, String> {
        let tls_dir = dir.join(TLS_DIR);
        match std::fs::symlink_metadata(&tls_dir) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(format!("TLS 证书目录不能是符号链接：{}", tls_dir.display()));
                }
                if !meta.is_dir() {
                    return Err(format!("TLS 证书路径不是目录：{}", tls_dir.display()));
                }
                if meta.uid() != current_euid() {
                    return Err(format!(
                        "TLS 证书目录的属主不是当前用户：{}",
                        tls_dir.display()
                    ));
                }
                Self::load_from_dir(&tls_dir)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::create_in(dir, &tls_dir),
            Err(e) => Err(format!(
                "读取 TLS 证书目录失败（{}）：{e}",
                tls_dir.display()
            )),
        }
    }

    pub fn server_config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.config)
    }

    fn load_from_dir(tls_dir: &Path) -> Result<Self, String> {
        let cert_path = tls_dir.join(CERT_FILE);
        let key_path = tls_dir.join(KEY_FILE);
        let cert_pem = read_pem_file(&cert_path, "证书", false)?;
        let key_pem = read_pem_file(&key_path, "私钥", true)?;
        Self::from_pem(&cert_pem, &key_pem)
    }

    fn create_in(parent: &Path, tls_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建证书父目录失败（{}）：{e}", parent.display()))?;

        let (cert_pem, key_pem) = generate_self_signed()?;
        // Validate the freshly generated pair before writing it out.
        let host_cert = Self::from_pem(cert_pem.as_bytes(), key_pem.as_bytes())?;

        // Write both files into a sibling temporary directory and rename it into place atomically so a crash
        // never leaves a half-populated `tls/` directory.
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_dir = parent.join(format!("tls.tmp-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&tmp_dir)
            .map_err(|e| format!("创建临时证书目录失败（{}）：{e}", tmp_dir.display()))?;

        let write_result = (|| -> Result<(), String> {
            write_file(&tmp_dir.join(CERT_FILE), cert_pem.as_bytes(), 0o644, "证书")?;
            write_file(&tmp_dir.join(KEY_FILE), key_pem.as_bytes(), 0o600, "私钥")?;
            Ok(())
        })();
        if let Err(e) = write_result {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(e);
        }

        if let Err(e) = std::fs::rename(&tmp_dir, tls_dir) {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            // Another thread or process may have created `tls/` concurrently.
            if tls_dir.is_dir() {
                return Self::load_from_dir(tls_dir);
            }
            return Err(format!(
                "保存 TLS 证书目录失败（{}）：{e}",
                tls_dir.display()
            ));
        }

        Ok(host_cert)
    }

    fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, String> {
        let cert_der = CertificateDer::from_pem_slice(cert_pem)
            .map_err(|e| format!("解析 TLS 证书失败：{e}"))?;
        let key_der = PrivateKeyDer::from_pem_slice(key_pem)
            .map_err(|e| format!("解析 TLS 私钥失败：{e}"))?;
        let config = build_server_config(&cert_der, &key_der)?;
        let fingerprint = cert_fingerprint(cert_der.as_ref());
        Ok(Self {
            cert_der,
            key_der,
            fingerprint,
            config,
        })
    }
}

fn current_euid() -> u32 {
    // SAFETY: `geteuid` has no preconditions.
    unsafe { libc::geteuid() }
}

fn read_pem_file(path: &Path, label: &str, require_0600: bool) -> Result<Vec<u8>, String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                format!("缺少 TLS {label}文件：{}", path.display())
            } else if e.raw_os_error() == Some(libc::ELOOP) {
                format!("TLS {label}不能是符号链接：{}", path.display())
            } else {
                format!("读取 TLS {label}失败（{}）：{e}", path.display())
            }
        })?;
    let meta = file
        .metadata()
        .map_err(|e| format!("检查 TLS {label}失败（{}）：{e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("TLS {label}不是普通文件：{}", path.display()));
    }
    if meta.uid() != current_euid() {
        return Err(format!("TLS {label}的属主不是当前用户：{}", path.display()));
    }
    if require_0600 {
        let mode = meta.permissions().mode() & 0o777;
        if (mode & !0o600) != 0 {
            return Err(format!(
                "TLS {label}权限过宽（当前 {:04o}，要求不超过 0600）：{}",
                mode,
                path.display()
            ));
        }
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("读取 TLS {label}失败（{}）：{e}", path.display()))?;
    Ok(buf)
}

fn write_file(path: &Path, bytes: &[u8], mode: u32, label: &str) -> Result<(), String> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|e| format!("写入 TLS {label}失败（{}）：{e}", path.display()))?;
    f.write_all(bytes)
        .map_err(|e| format!("写入 TLS {label}失败（{}）：{e}", path.display()))
}

fn generate_self_signed() -> Result<(String, String), String> {
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|e| format!("生成 TLS 私钥失败：{e}"))?;
    let mut params = rcgen::CertificateParams::default();
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "CoDeck Host");
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];

    let today = chrono::Utc::now().date_naive();
    let start = today - chrono::Days::new(1);
    let end = today + chrono::Days::new(CERT_VALID_DAYS);
    params.not_before = rcgen::date_time_ymd(start.year(), start.month() as u8, start.day() as u8);
    params.not_after = rcgen::date_time_ymd(end.year(), end.month() as u8, end.day() as u8);

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| format!("生成 TLS 证书失败：{e}"))?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}

fn build_server_config(
    cert_der: &CertificateDer<'static>,
    key_der: &PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, String> {
    // Explicit ring provider so we never depend on a process-wide default CryptoProvider.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("初始化 TLS 配置失败：{e}"))?
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key_der.clone_key())
        .map_err(|e| format!("TLS 证书与私钥无效或不匹配：{e}"))?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

/// Shared connection budget across TLS listeners (total and per-source-IP).
pub struct ConnLimits {
    max_total: usize,
    max_per_ip: usize,
    counts: Mutex<ConnCounts>,
}

#[derive(Default)]
struct ConnCounts {
    total: usize,
    per_ip: HashMap<IpAddr, usize>,
}

struct ConnGuard {
    limits: Arc<ConnLimits>,
    ip: IpAddr,
}

static PROCESS_CONN_LIMITS: OnceLock<Arc<ConnLimits>> = OnceLock::new();

/// Process-wide shared TLS connection quota (`MAX_CONNECTIONS` total, `MAX_CONNECTIONS_PER_IP` per source IP).
pub fn process_conn_limits() -> Arc<ConnLimits> {
    Arc::clone(
        PROCESS_CONN_LIMITS
            .get_or_init(|| ConnLimits::new(MAX_CONNECTIONS, MAX_CONNECTIONS_PER_IP)),
    )
}

impl ConnLimits {
    pub fn new(max_total: usize, max_per_ip: usize) -> Arc<Self> {
        Arc::new(Self {
            max_total,
            max_per_ip,
            counts: Mutex::new(ConnCounts::default()),
        })
    }

    fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnGuard> {
        let ip = ip.to_canonical();
        let mut counts = self.counts.lock();
        let cur_ip = counts.per_ip.get(&ip).copied().unwrap_or(0);
        if counts.total >= self.max_total || cur_ip >= self.max_per_ip {
            return None;
        }
        counts.total += 1;
        *counts.per_ip.entry(ip).or_insert(0) += 1;
        Some(ConnGuard {
            limits: Arc::clone(self),
            ip,
        })
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        let mut counts = self.limits.counts.lock();
        counts.total = counts.total.saturating_sub(1);
        if let Some(c) = counts.per_ip.get_mut(&self.ip) {
            *c -= 1;
            if *c == 0 {
                counts.per_ip.remove(&self.ip);
            }
        }
    }
}

/// A [`TlsStream`] whose lifetime holds a connection-limit permit until dropped and enforces an
/// idle read deadline reset whenever any decrypted byte is read.
pub struct TrackedTlsStream {
    inner: TlsStream<TcpStream>,
    _guard: ConnGuard,
    idle_timeout: Duration,
    idle_sleep: Pin<Box<tokio::time::Sleep>>,
}

impl TrackedTlsStream {
    fn new(inner: TlsStream<TcpStream>, guard: ConnGuard, idle_timeout: Duration) -> Self {
        Self {
            inner,
            _guard: guard,
            idle_timeout,
            idle_sleep: Box::pin(tokio::time::sleep(idle_timeout)),
        }
    }
}

impl AsyncRead for TrackedTlsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    let deadline = tokio::time::Instant::now() + self.idle_timeout;
                    self.idle_sleep.as_mut().reset(deadline);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => match self.idle_sleep.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "TLS connection idle read timeout",
                ))),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

impl AsyncWrite for TrackedTlsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

type HandshakeFuture = Pin<Box<dyn Future<Output = Option<(TrackedTlsStream, SocketAddr)>> + Send>>;

/// An [`axum::serve::Listener`] that performs TLS handshakes concurrently inside `accept()` without
/// spawning a background task, so dropping the listener frees the TCP socket and cancels in-flight handshakes.
pub struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    handshakes: FuturesUnordered<HandshakeFuture>,
    limits: Arc<ConnLimits>,
    idle_read_timeout: Duration,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>) -> Self {
        Self::with_shared_limits(tcp, config, process_conn_limits(), IDLE_READ_TIMEOUT)
    }

    pub fn with_limits(
        tcp: TcpListener,
        config: Arc<ServerConfig>,
        max_connections: usize,
        max_per_ip: usize,
    ) -> Self {
        Self::with_shared_limits(
            tcp,
            config,
            ConnLimits::new(max_connections, max_per_ip),
            IDLE_READ_TIMEOUT,
        )
    }

    pub fn with_shared_limits(
        tcp: TcpListener,
        config: Arc<ServerConfig>,
        limits: Arc<ConnLimits>,
        idle_read_timeout: Duration,
    ) -> Self {
        Self {
            tcp,
            acceptor: TlsAcceptor::from(config),
            handshakes: FuturesUnordered::new(),
            limits,
            idle_read_timeout,
        }
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TrackedTlsStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                biased;
                Some(done) = self.handshakes.next(), if !self.handshakes.is_empty() => {
                    if let Some(conn) = done {
                        return conn;
                    }
                }
                res = self.tcp.accept(), if self.handshakes.len() < MAX_HANDSHAKES => {
                    match res {
                        Ok((stream, peer)) => {
                            let Some(guard) = self.limits.try_acquire(peer.ip()) else {
                                tracing::debug!("dropping connection from {peer}: connection limit reached");
                                drop(stream);
                                continue;
                            };
                            let acceptor = self.acceptor.clone();
                            let idle_timeout = self.idle_read_timeout;
                            self.handshakes.push(Box::pin(async move {
                                match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                                    Ok(Ok(tls)) => Some((TrackedTlsStream::new(tls, guard, idle_timeout), peer)),
                                    Ok(Err(e)) => {
                                        tracing::debug!("tls handshake from {peer} failed: {e}");
                                        None
                                    }
                                    Err(_) => {
                                        tracing::debug!("tls handshake from {peer} timed out");
                                        None
                                    }
                                }
                            }));
                        }
                        Err(e) => handle_accept_error(e).await,
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

/// Mirrors axum's `TcpListener` accept error handling: retry immediately on peer resets, back off briefly on
/// resource errors such as `EMFILE`.
async fn handle_accept_error(e: io::Error) {
    if matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    ) {
        return;
    }
    tracing::error!("accept error: {e}");
    tokio::time::sleep(Duration::from_secs(1)).await;
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn load_or_create_is_stable_and_sets_key_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let first = HostCert::load_or_create(dir.path()).unwrap();
        assert_eq!(first.fingerprint.len(), 43);
        assert!(
            first
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "{}",
            first.fingerprint
        );

        let key_path = dir.path().join("tls").join("key.pem");
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let second = HostCert::load_or_create(dir.path()).unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(first.cert_der.as_ref(), second.cert_der.as_ref());
        assert_eq!(first.server_config().alpn_protocols, [b"http/1.1".to_vec()]);
    }

    #[test]
    fn incomplete_or_mismatched_tls_dir_errors_without_regenerating() {
        let dir = tempfile::tempdir().unwrap();
        let orig = HostCert::load_or_create(dir.path()).unwrap();
        let tls_dir = dir.path().join("tls");
        let cert_path = tls_dir.join("cert.pem");
        let key_path = tls_dir.join("key.pem");
        let orig_cert_pem = std::fs::read(&cert_path).unwrap();

        // Missing key.pem must fail and must not overwrite or regenerate.
        std::fs::remove_file(&key_path).unwrap();
        let err = HostCert::load_or_create(dir.path()).unwrap_err();
        assert!(err.contains("私钥"), "{err}");
        assert!(!key_path.exists());
        assert_eq!(std::fs::read(&cert_path).unwrap(), orig_cert_pem);

        // Mismatched key.pem from another certificate must fail too.
        let other = tempfile::tempdir().unwrap();
        let other_cert = HostCert::load_or_create(other.path()).unwrap();
        assert_ne!(orig.fingerprint, other_cert.fingerprint);
        std::fs::copy(other.path().join("tls/key.pem"), &key_path).unwrap();
        let err = HostCert::load_or_create(dir.path()).unwrap_err();
        assert!(err.contains("不匹配") || err.contains("无效"), "{err}");
        assert_eq!(std::fs::read(&cert_path).unwrap(), orig_cert_pem);
    }

    #[test]
    fn rejects_wide_key_permissions_and_symlinks_without_modifying() {
        let dir = tempfile::tempdir().unwrap();
        let _orig = HostCert::load_or_create(dir.path()).unwrap();
        let tls_dir = dir.path().join("tls");
        let key_path = tls_dir.join("key.pem");

        // Key permissions wider than 0600 must error and stay untouched.
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = HostCert::load_or_create(dir.path()).unwrap_err();
        assert!(err.contains("权限过宽") && err.contains("0600"), "{err}");
        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o644
        );

        // Symlinked key.pem must be rejected.
        let real_key = dir.path().join("real-key.pem");
        std::fs::rename(&key_path, &real_key).unwrap();
        std::fs::set_permissions(&real_key, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real_key, &key_path).unwrap();
        let err = HostCert::load_or_create(dir.path()).unwrap_err();
        assert!(err.contains("符号链接"), "{err}");

        // Symlinked tls directory must be rejected.
        let link_root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&tls_dir, link_root.path().join("tls")).unwrap();
        let err = HostCert::load_or_create(link_root.path()).unwrap_err();
        assert!(err.contains("符号链接"), "{err}");
    }

    #[test]
    fn conn_limits_enforce_per_ip_and_total_and_release_on_drop() {
        let limits = ConnLimits::new(3, 2);
        let ip_a = IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 10));
        let ip_b = IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 11));

        let g1 = limits.try_acquire(ip_a).unwrap();
        let g2 = limits.try_acquire(ip_a).unwrap();
        assert!(limits.try_acquire(ip_a).is_none());

        // Another IP still gets a slot until total limit is reached.
        let g3 = limits.try_acquire(ip_b).unwrap();
        assert!(limits.try_acquire(ip_b).is_none());

        drop(g1);
        let _g4 = limits.try_acquire(ip_a).unwrap();
        drop(g2);
        drop(g3);
        let _g5 = limits.try_acquire(ip_b).unwrap();
    }
}
