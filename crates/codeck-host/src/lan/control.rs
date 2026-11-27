//! Daemon control socket (`<state-dir>/control.sock`), single-instance lock (`daemon.lock`), and
//! CLI formatting helpers for `codeck-host pair / devices / unpair`.

use std::fs::{File, OpenOptions, Permissions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use super::devices::PairedDevice;
use super::pairing::{PAIRING_TTL, PairOutcome, build_pairing_url, private_ipv4_addrs};
use super::security::LanSecurity;

pub const LOCK_FILE: &str = "daemon.lock";
pub const SOCKET_FILE: &str = "control.sock";
pub const MAX_REQUEST_LINE_BYTES: usize = 4 * 1024;
pub const FIRST_LINE_TIMEOUT: Duration = Duration::from_secs(10);

/// Holds an exclusive `flock` on `<state-dir>/daemon.lock` for the lifetime of the daemon.
pub struct DaemonLock {
    _file: File,
}

/// Ensures `state_dir` exists with mode `0700` and acquires a non-blocking exclusive `flock` on
/// `<state-dir>/daemon.lock`.
pub fn lock_state_dir(state_dir: &Path) -> Result<DaemonLock, String> {
    std::fs::create_dir_all(state_dir)
        .map_err(|e| format!("创建状态目录失败（{}）：{e}", state_dir.display()))?;
    std::fs::set_permissions(state_dir, Permissions::from_mode(0o700))
        .map_err(|e| format!("设置状态目录权限失败（{}）：{e}", state_dir.display()))?;

    let lock_path = state_dir.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|e| format!("打开锁文件失败（{}）：{e}", lock_path.display()))?;

    // SAFETY: `file.as_raw_fd()` is a valid open file descriptor owned by `file`.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            return Err(format!(
                "另一个 codeck-host 正在使用 {}",
                state_dir.display()
            ));
        }
        return Err(format!(
            "锁定状态目录失败（{}）：{err}",
            state_dir.display()
        ));
    }
    Ok(DaemonLock { _file: file })
}

/// Bound `<state-dir>/control.sock`; removes the socket file when dropped.
pub struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl ControlSocket {
    /// Removes a stale socket file if present (refusing non-socket files), binds `control.sock`, and
    /// restricts its permissions to `0600`. Call only after holding [`DaemonLock`].
    pub fn bind(state_dir: &Path) -> Result<Self, String> {
        let path = state_dir.join(SOCKET_FILE);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                if !meta.file_type().is_socket() {
                    return Err(format!("{} 已存在且不是 socket 文件", path.display()));
                }
                std::fs::remove_file(&path)
                    .map_err(|e| format!("删除残留 socket 失败（{}）：{e}", path.display()))?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("检查 socket 失败（{}）：{e}", path.display())),
        }

        let listener = UnixListener::bind(&path)
            .map_err(|e| format!("监听控制 socket 失败（{}）：{e}", path.display()))?;
        std::fs::set_permissions(&path, Permissions::from_mode(0o600))
            .map_err(|e| format!("设置控制 socket 权限失败（{}）：{e}", path.display()))?;
        Ok(Self { listener, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs the accept loop until the returned future is dropped or cancelled.
    pub async fn serve(&self, ctx: ControlCtx) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, ctx).await;
                    });
                }
                Err(e) => {
                    tracing::warn!("control socket accept error: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Live listening `address:port` snapshot provider (reads directly from `Serving`).
pub type ListeningProvider = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

#[derive(Clone)]
pub struct ControlCtx {
    pub security: Arc<LanSecurity>,
    pub port: u16,
    pub listening: ListeningProvider,
    pub first_line_timeout: Duration,
    pub pairing_ttl: Duration,
}

impl ControlCtx {
    pub fn new(security: Arc<LanSecurity>, port: u16, listening: ListeningProvider) -> Self {
        Self {
            security,
            port,
            listening,
            first_line_timeout: FIRST_LINE_TIMEOUT,
            pairing_ttl: PAIRING_TTL,
        }
    }
}

#[derive(Deserialize)]
struct ControlRequest {
    cmd: String,
    #[serde(default)]
    id: Option<String>,
}

async fn write_json_line(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    value: &Value,
) -> io::Result<()> {
    let mut line = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}

/// Reads a single `\n`-terminated line of at most `max_bytes` bytes within `timeout`. Returns `None`
/// on timeout, EOF, I/O error, or oversize line.
async fn read_bounded_line(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    max_bytes: usize,
    timeout: Duration,
) -> Option<String> {
    let mut line = String::new();
    let res = tokio::time::timeout(timeout, async {
        reader
            .take((max_bytes + 1) as u64)
            .read_line(&mut line)
            .await
    })
    .await;
    match res {
        Ok(Ok(0)) | Ok(Err(_)) | Err(_) => None,
        Ok(Ok(_)) => {
            if line.len() > max_bytes || !line.ends_with('\n') {
                return None;
            }
            Some(line)
        }
    }
}

pub async fn handle_connection(stream: UnixStream, ctx: ControlCtx) {
    let (read_half, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let Some(line) =
        read_bounded_line(&mut reader, MAX_REQUEST_LINE_BYTES, ctx.first_line_timeout).await
    else {
        return;
    };
    let req: ControlRequest = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            let _ =
                write_json_line(&mut writer, &json!({ "error": format!("无效请求：{e}") })).await;
            return;
        }
    };

    match req.cmd.as_str() {
        "devices" => {
            let devices = ctx.security.devices();
            let _ = write_json_line(&mut writer, &json!({ "devices": devices })).await;
        }
        "unpair" => {
            let Some(id) = req.id.filter(|s| !s.is_empty()) else {
                let _ = write_json_line(&mut writer, &json!({ "error": "缺少设备 ID" })).await;
                return;
            };
            match ctx.security.remove_device(&id) {
                Ok((Some(dev), _)) => {
                    let _ = write_json_line(
                        &mut writer,
                        &json!({ "removed": { "id": dev.id, "name": dev.name } }),
                    )
                    .await;
                }
                Ok((None, _)) => {
                    let _ = write_json_line(&mut writer, &json!({ "notFound": true })).await;
                }
                Err(e) => {
                    let _ = write_json_line(
                        &mut writer,
                        &json!({ "error": format!("移除设备失败：{e}") }),
                    )
                    .await;
                }
            }
        }
        "pair" => handle_pair(&mut reader, &mut writer, &ctx).await,
        other => {
            let _ = write_json_line(
                &mut writer,
                &json!({ "error": format!("不认识的命令：{other}") }),
            )
            .await;
        }
    }
}

async fn handle_pair(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    ctx: &ControlCtx,
) {
    let addrs = (ctx.listening)();
    let hosts = private_ipv4_addrs(&addrs);
    if hosts.is_empty() {
        let _ = write_json_line(writer, &json!({ "error": "没有可用的局域网地址。" })).await;
        return;
    }
    let cert = match ctx.security.ensure_cert() {
        Ok(c) => c,
        Err(e) => {
            let _ = write_json_line(writer, &json!({ "error": e })).await;
            return;
        }
    };
    let started = match ctx.security.start_pairing_with_ttl(
        Instant::now(),
        chrono::Utc::now().timestamp_millis(),
        ctx.pairing_ttl,
    ) {
        Ok(o) => o,
        Err(e) => {
            let _ = write_json_line(writer, &json!({ "error": e })).await;
            return;
        }
    };
    let Some(url) = build_pairing_url(&hosts, ctx.port, &cert.fingerprint, &started.key) else {
        ctx.security.cancel_pairing(&started.id);
        let _ = write_json_line(writer, &json!({ "error": "没有可用的局域网地址。" })).await;
        return;
    };

    if write_json_line(
        writer,
        &json!({ "url": url, "expiresAt": started.expires_at_ms }),
    )
    .await
    .is_err()
    {
        ctx.security.cancel_pairing(&started.id);
        return;
    }

    let mut disconnect_buf = [0u8; 16];
    let deadline = tokio::time::Instant::from_std(started.deadline);
    tokio::select! {
        res = started.outcome => match res {
            Ok(PairOutcome::Paired(dev)) => {
                let _ = write_json_line(
                    writer,
                    &json!({ "paired": { "id": dev.id, "name": dev.name } }),
                )
                .await;
            }
            Ok(PairOutcome::Expired) | Err(_) => {
                let _ = write_json_line(writer, &json!({ "expired": true })).await;
            }
        },
        _ = tokio::time::sleep_until(deadline) => {
            ctx.security.cancel_pairing(&started.id);
            let _ = write_json_line(writer, &json!({ "expired": true })).await;
        }
        // Client disconnected (e.g. Ctrl-C) or sent unexpected trailing bytes: cancel its own offer.
        _ = reader.read(&mut disconnect_buf) => {
            ctx.security.cancel_pairing(&started.id);
        }
    }
}

/// Replaces control characters (including newlines, tabs, ANSI escapes) with `?` so each device
/// prints cleanly on a single terminal line.
pub fn sanitize_display_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Renders a pairing URL using Unicode half-blocks (`Dense1x2`) with an inverted quiet zone so it
/// scans cleanly on dark terminals (and standard scanners).
pub fn render_qr_unicode(url: &str) -> Result<String, String> {
    use qrcode::render::unicode::Dense1x2;
    let code =
        qrcode::QrCode::new(url.as_bytes()).map_err(|e| format!("生成配对二维码失败：{e}"))?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

/// Formats an epoch millisecond timestamp in the local timezone as `yyyy-MM-dd HH:mm`.
pub fn format_local_time(ms: f64) -> String {
    match Local.timestamp_millis_opt(ms as i64).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        None => "-".into(),
    }
}

/// Formats one row of `codeck-host devices`.
pub fn format_device_line(dev: &PairedDevice) -> String {
    let name = sanitize_display_name(&dev.name);
    let paired = format_local_time(dev.paired_at);
    let last_seen = dev
        .last_seen_at
        .map_or_else(|| "从未".to_string(), format_local_time);
    format!("{}  {name}  配对于 {paired}  最近连接 {last_seen}", dev.id)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::lan::security::RedeemError;

    fn parse_key(url: &str) -> String {
        let q = url.strip_prefix("codeck://pair?").unwrap();
        let map: HashMap<_, _> = q.split('&').filter_map(|kv| kv.split_once('=')).collect();
        (*map.get("k").unwrap()).to_string()
    }

    async fn read_json_line(reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Value {
        let mut line = String::new();
        let n = tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut line))
            .await
            .expect("timed out reading control response")
            .unwrap();
        assert!(n > 0, "unexpected EOF");
        serde_json::from_str(line.trim()).unwrap()
    }

    #[test]
    fn flock_rejects_second_holder_on_same_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        let lock1 = lock_state_dir(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let err = lock_state_dir(dir.path()).err().unwrap();
        assert!(err.contains("另一个 codeck-host 正在使用"), "{err}");

        drop(lock1);
        let _lock2 = lock_state_dir(dir.path()).unwrap();
    }

    #[test]
    fn sanitize_replaces_control_chars_and_formats_device_line() {
        assert_eq!(
            sanitize_display_name("iPhone\n\x1b[31mEvil\tName\x7f"),
            "iPhone??[31mEvil?Name?"
        );
        let dev = PairedDevice {
            id: "0123456789ab".into(),
            name: "My\nPhone".into(),
            paired_at: 1_700_000_000_000.0,
            last_seen_at: None,
        };
        let line = format_device_line(&dev);
        assert!(!line.contains('\n'), "{line}");
        assert!(
            line.starts_with("0123456789ab  My?Phone  配对于 "),
            "{line}"
        );
        assert!(line.ends_with("  最近连接 从未"), "{line}");

        let qr = render_qr_unicode("codeck://pair?h=192.168.1.20&p=38471&fp=abc&k=xyz").unwrap();
        assert!(qr.contains('\u{2588}') || qr.contains('\u{2580}') || qr.contains('\u{2584}'));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn control_socket_pair_supersede_disconnect_devices_unpair_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = lock_state_dir(dir.path()).unwrap();
        let sec = Arc::new(LanSecurity::open(dir.path().to_path_buf()).unwrap());
        sec.ensure_cert().unwrap();

        let sock = Arc::new(ControlSocket::bind(dir.path()).unwrap());
        let sock_mode = std::fs::metadata(sock.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(sock_mode, 0o600);

        let ctx = ControlCtx {
            security: Arc::clone(&sec),
            port: 38471,
            listening: Arc::new(|| vec!["192.168.1.20:38471".into()]),
            first_line_timeout: Duration::from_millis(200),
            pairing_ttl: Duration::from_secs(60),
        };
        let sock_path = sock.path().to_path_buf();
        let server = {
            let sock = Arc::clone(&sock);
            tokio::spawn(async move { sock.serve(ctx).await })
        };

        // 1. Pair client 1, then pair client 2 -> client 1 gets `expired`; redeem client 2 -> client 2 gets `paired`.
        let (r1, mut w1) = UnixStream::connect(&sock_path).await.unwrap().into_split();
        let mut r1 = BufReader::new(r1);
        w1.write_all(b"{\"cmd\":\"pair\"}\n").await.unwrap();
        let line1 = read_json_line(&mut r1).await;
        assert!(
            line1["url"]
                .as_str()
                .unwrap()
                .starts_with("codeck://pair?h=192.168.1.20&p=38471&")
        );
        assert!(line1["expiresAt"].is_number());

        let (r2, mut w2) = UnixStream::connect(&sock_path).await.unwrap().into_split();
        let mut r2 = BufReader::new(r2);
        w2.write_all(b"{\"cmd\":\"pair\"}\n").await.unwrap();
        let line2 = read_json_line(&mut r2).await;
        let key2 = parse_key(line2["url"].as_str().unwrap());

        // Client 1 receives expired as soon as client 2 starts a new offer.
        let exp1 = read_json_line(&mut r1).await;
        assert_eq!(exp1, json!({ "expired": true }));

        // Redeem client 2's key via LanSecurity -> client 2 receives `paired`.
        let redeemed = sec.redeem(&key2, "My iPhone").unwrap();
        let paired2 = read_json_line(&mut r2).await;
        assert_eq!(
            paired2,
            json!({ "paired": { "id": redeemed.device.id, "name": "My iPhone" } })
        );

        // 2. Client 3 starts a pair and disconnects -> its offer is cancelled.
        let (r3, mut w3) = UnixStream::connect(&sock_path).await.unwrap().into_split();
        let mut r3 = BufReader::new(r3);
        w3.write_all(b"{\"cmd\":\"pair\"}\n").await.unwrap();
        let line3 = read_json_line(&mut r3).await;
        let key3 = parse_key(line3["url"].as_str().unwrap());
        assert!(sec.has_active_pairing());
        drop(r3);
        drop(w3);
        // Wait for the server task to observe EOF and cancel the offer before attempting redeem.
        let deadline = Instant::now() + Duration::from_secs(2);
        while sec.has_active_pairing() {
            assert!(Instant::now() < deadline, "disconnect did not cancel offer");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(matches!(
            sec.redeem(&key3, "Late"),
            Err(RedeemError::Expired)
        ));

        // 3. `devices` lists the paired device; `unpair` removes it and reports notFound on unknown id.
        let (rd, mut wd) = UnixStream::connect(&sock_path).await.unwrap().into_split();
        let mut rd = BufReader::new(rd);
        wd.write_all(b"{\"cmd\":\"devices\"}\n").await.unwrap();
        let devs = read_json_line(&mut rd).await;
        assert_eq!(devs["devices"].as_array().unwrap().len(), 1);
        assert_eq!(devs["devices"][0]["id"], redeemed.device.id);

        let (ru1, mut wu1) = UnixStream::connect(&sock_path).await.unwrap().into_split();
        let mut ru1 = BufReader::new(ru1);
        wu1.write_all(b"{\"cmd\":\"unpair\",\"id\":\"nope\"}\n")
            .await
            .unwrap();
        assert_eq!(read_json_line(&mut ru1).await, json!({ "notFound": true }));

        let (ru2, mut wu2) = UnixStream::connect(&sock_path).await.unwrap().into_split();
        let mut ru2 = BufReader::new(ru2);
        wu2.write_all(
            format!("{{\"cmd\":\"unpair\",\"id\":\"{}\"}}\n", redeemed.device.id).as_bytes(),
        )
        .await
        .unwrap();
        assert_eq!(
            read_json_line(&mut ru2).await,
            json!({ "removed": { "id": redeemed.device.id, "name": "My iPhone" } })
        );

        // 4. Oversize first line (> 4 KiB) is disconnected without a response.
        let (mut r_big, mut w_big) = UnixStream::connect(&sock_path).await.unwrap().into_split();
        let junk = vec![b'a'; MAX_REQUEST_LINE_BYTES + 128];
        let _ = w_big.write_all(&junk).await;
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), r_big.read_to_end(&mut buf))
            .await
            .expect("oversize client was not disconnected")
            .unwrap();
        assert!(buf.is_empty());

        // 5. Slow client that never finishes the first line is disconnected after `first_line_timeout`.
        let (mut r_slow, mut w_slow) = UnixStream::connect(&sock_path).await.unwrap().into_split();
        w_slow.write_all(b"{\"cmd\":\"dev").await.unwrap();
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), r_slow.read_to_end(&mut buf))
            .await
            .expect("slow client was not disconnected")
            .unwrap();
        assert!(buf.is_empty());

        // 6. Dropping the socket removes `control.sock`.
        server.abort();
        let _ = server.await;
        drop(sock);
        assert!(!sock_path.exists());
    }

    #[test]
    fn bind_refuses_non_socket_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(SOCKET_FILE), "not a socket").unwrap();
        let err = ControlSocket::bind(dir.path()).err().unwrap();
        assert!(err.contains("不是 socket"), "{err}");
    }
}
