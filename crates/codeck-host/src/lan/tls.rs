//! Self-signed TLS certificate persistence and the axum [`TlsListener`] for non-loopback LAN listeners.

use std::future::Future;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine;
use chrono::Datelike;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
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
                if !meta.is_dir() {
                    return Err(format!("TLS 证书路径不是目录：{}", tls_dir.display()));
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
        let cert_pem = read_pem_file(&cert_path, "证书")?;
        let key_pem = read_pem_file(&key_path, "私钥")?;
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

fn read_pem_file(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            format!("缺少 TLS {label}文件：{}", path.display())
        } else {
            format!("读取 TLS {label}失败（{}）：{e}", path.display())
        }
    })
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

type HandshakeFuture =
    Pin<Box<dyn Future<Output = Option<(TlsStream<TcpStream>, SocketAddr)>> + Send>>;

/// An [`axum::serve::Listener`] that performs TLS handshakes concurrently inside `accept()` without
/// spawning a background task, so dropping the listener frees the TCP socket and cancels in-flight handshakes.
pub struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    handshakes: FuturesUnordered<HandshakeFuture>,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>) -> Self {
        Self {
            tcp,
            acceptor: TlsAcceptor::from(config),
            handshakes: FuturesUnordered::new(),
        }
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
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
                            let acceptor = self.acceptor.clone();
                            self.handshakes.push(Box::pin(async move {
                                match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                                    Ok(Ok(tls)) => Some((tls, peer)),
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
}
