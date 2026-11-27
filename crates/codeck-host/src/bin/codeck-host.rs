//! `codeck-host`: CoDeck's headless Host (ADR-0011). Indexes a data root like the desktop app and serves
//! the Host's `remote` methods and events over HTTPS + WebSocket (plain HTTP on loopback) to the LAN.

use std::io::{BufRead, BufReader, Write};
use std::net::Ipv4Addr;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use codeck_host::lan::{DEFAULT_PORT, PairedDevice, control, net, security, server};
use codeck_host::{Host, HostConfig};
use serde_json::{Value, json};

/// How often the default listeners are checked against the machine's addresses.
const NETWORK_POLL: std::time::Duration = std::time::Duration::from_secs(5);

const USAGE: &str = "用法：codeck-host [--port <端口>] [--listen <IPv4>]… [--state-dir <目录>] [--data-root <目录>] [--web-dir <目录>] [--allow-origin <origin>]…
      codeck-host pair [--state-dir <目录>]
      codeck-host devices [--state-dir <目录>]
      codeck-host unpair <id> [--state-dir <目录>]

  --port          端口，默认 38471
  --listen        监听的 IPv4 地址，可重复；默认是所有已启用的局域网网卡（en*、eth*、wl*）的私网地址，随网络变化增减；其他网卡与回环地址只在这里指定时监听
  --state-dir     索引、日志、图片、证书与设备列表所在的目录
  --data-root     Claude Code 的数据目录；默认依次取 settings.json、$CLAUDE_CONFIG_DIR、~/.claude
  --web-dir       /web/ 下提供的静态文件
  --allow-origin  允许跨域访问的网页来源，可重复

子命令：
  pair            生成配对二维码与链接，等待设备扫码完成配对
  devices         列出已配对的设备
  unpair <id>     移除已配对的设备并立即断开其连接";

#[derive(Debug, Default, PartialEq)]
struct Args {
    port: Option<u16>,
    listen: Vec<Ipv4Addr>,
    state_dir: Option<PathBuf>,
    data_root: Option<PathBuf>,
    web_dir: Option<PathBuf>,
    allow_origin: Vec<String>,
}

#[derive(Debug, PartialEq)]
enum Command {
    Daemon(Args),
    Pair {
        state_dir: Option<PathBuf>,
    },
    Devices {
        state_dir: Option<PathBuf>,
    },
    Unpair {
        id: String,
        state_dir: Option<PathBuf>,
    },
}

/// `None` for `--help`.
fn parse_command(mut it: impl Iterator<Item = String>) -> Result<Option<Command>, String> {
    let Some(first) = it.next() else {
        return Ok(Some(Command::Daemon(Args::default())));
    };
    if first == "-h" || first == "--help" {
        return Ok(None);
    }
    match first.as_str() {
        "pair" => Ok(parse_sub_state_dir(it)?.map(|state_dir| Command::Pair { state_dir })),
        "devices" => Ok(parse_sub_state_dir(it)?.map(|state_dir| Command::Devices { state_dir })),
        "unpair" => parse_unpair_args(it),
        _ => Ok(parse_daemon_args(std::iter::once(first).chain(it))?.map(Command::Daemon)),
    }
}

fn parse_sub_state_dir(
    mut it: impl Iterator<Item = String>,
) -> Result<Option<Option<PathBuf>>, String> {
    let mut state_dir = None;
    while let Some(arg) = it.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(None);
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg, None),
        };
        match flag.as_str() {
            "--state-dir" => {
                let v = inline
                    .or_else(|| it.next())
                    .ok_or_else(|| "--state-dir 缺少参数".to_string())?;
                state_dir = Some(PathBuf::from(v));
            }
            _ => return Err(format!("不认识的参数：{flag}")),
        }
    }
    Ok(Some(state_dir))
}

fn parse_unpair_args(mut it: impl Iterator<Item = String>) -> Result<Option<Command>, String> {
    let mut state_dir = None;
    let mut id = None;
    while let Some(arg) = it.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(None);
        }
        if arg.starts_with("--") {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (arg, None),
            };
            match flag.as_str() {
                "--state-dir" => {
                    let v = inline
                        .or_else(|| it.next())
                        .ok_or_else(|| "--state-dir 缺少参数".to_string())?;
                    state_dir = Some(PathBuf::from(v));
                }
                _ => return Err(format!("不认识的参数：{flag}")),
            }
        } else if id.is_none() {
            id = Some(arg);
        } else {
            return Err(format!("多余的参数：{arg}"));
        }
    }
    let id = id.ok_or_else(|| "unpair 缺少设备 ID".to_string())?;
    Ok(Some(Command::Unpair { id, state_dir }))
}

/// `None` for `--help`.
fn parse_daemon_args(mut it: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut a = Args::default();
    while let Some(arg) = it.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(None);
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg, None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{flag} 缺少参数"))
        };
        match flag.as_str() {
            "--port" => {
                let v = value()?;
                let port: u16 = v.parse().map_err(|_| format!("端口不对：{v}"))?;
                if port == 0 {
                    return Err("端口不能为 0".into());
                }
                a.port = Some(port);
            }
            "--listen" => {
                let v = value()?;
                a.listen
                    .push(v.parse().map_err(|_| format!("不是 IPv4 地址：{v}"))?);
            }
            "--state-dir" => a.state_dir = Some(value()?.into()),
            "--data-root" => a.data_root = Some(value()?.into()),
            "--web-dir" => a.web_dir = Some(value()?.into()),
            "--allow-origin" => a
                .allow_origin
                .push(value()?.trim_end_matches('/').to_string()),
            _ => return Err(format!("不认识的参数：{flag}")),
        }
    }
    Ok(Some(a))
}

/// macOS: `~/Library/Application Support/dev.joy.codeck.host`; elsewhere `$XDG_STATE_HOME/codeck`
/// (`~/.local/state/codeck`).
fn default_state_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("{}.host", codeck_core::paths::BUNDLE_ID))
    } else {
        dirs::state_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
            .unwrap_or_else(std::env::temp_dir)
            .join("codeck")
    }
}

/// `<exe dir>/web`; when that is missing, the repository's `web/dist-web` (a development build).
fn default_web_dir() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("web")))
        .unwrap_or_else(|| PathBuf::from("web"));
    let dev = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/dist-web"));
    if !beside.is_dir() && dev.is_dir() {
        return dev;
    }
    beside
}

fn needs_tls_cert(follow_network: bool, subnets: &[net::Subnet]) -> bool {
    follow_network || subnets.iter().any(server::needs_tls)
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("codeck-host: {msg}");
    std::process::exit(1);
}

fn resolve_state_dir(raw: Option<PathBuf>) -> PathBuf {
    let raw = raw.unwrap_or_else(default_state_dir);
    control::normalize_state_dir(&raw).unwrap_or_else(|e| fail(e))
}

fn connect_control(state_dir: &Path) -> UnixStream {
    let sock_path = control::check_socket_path_len(state_dir).unwrap_or_else(|e| fail(e));
    let stream = UnixStream::connect(&sock_path).unwrap_or_else(|_| {
        eprintln!("codeck-host 没有在运行（{}）", state_dir.display());
        std::process::exit(1);
    });
    control::verify_peer_euid(&stream).unwrap_or_else(|e| fail(e));
    stream
}

fn send_request(stream: &mut UnixStream, req: &Value) {
    let mut line = serde_json::to_string(req).unwrap();
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.flush())
        .unwrap_or_else(|e| fail(format!("发送控制请求失败：{e}")));
}

fn read_response(reader: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => Some(
            serde_json::from_str(line.trim())
                .unwrap_or_else(|e| fail(format!("解析控制应答失败：{e}"))),
        ),
        Err(e) => fail(format!("读取控制应答失败：{e}")),
    }
}

fn run_pair(state_dir: PathBuf) -> ! {
    let mut stream = connect_control(&state_dir);
    send_request(&mut stream, &json!({ "cmd": "pair" }));
    let mut reader = BufReader::new(stream);

    let first = read_response(&mut reader).unwrap_or_else(|| fail("与 codeck-host 的连接已断开"));
    if let Some(err) = first.get("error").and_then(Value::as_str) {
        fail(err);
    }
    let url = first
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("控制应答缺少配对链接"));
    let qr = control::render_qr_unicode(url).unwrap_or_else(|e| fail(e));
    println!("{qr}\n{url}\n5 分钟内有效");
    let _ = std::io::stdout().flush();

    match read_response(&mut reader) {
        Some(v) if v.get("paired").is_some() => {
            let name = v["paired"]["name"].as_str().unwrap_or("iPhone");
            println!("已配对 {}", control::sanitize_display_name(name));
            std::process::exit(0);
        }
        Some(v) if v.get("expired").and_then(Value::as_bool) == Some(true) => {
            eprintln!("配对码已过期");
            std::process::exit(1);
        }
        Some(v) if v.get("error").is_some() => {
            fail(v["error"].as_str().unwrap_or("配对失败"));
        }
        _ => {
            eprintln!("配对码已过期");
            std::process::exit(1);
        }
    }
}

fn run_devices(state_dir: PathBuf) -> ! {
    let mut stream = connect_control(&state_dir);
    send_request(&mut stream, &json!({ "cmd": "devices" }));
    let mut reader = BufReader::new(stream);
    let resp = read_response(&mut reader).unwrap_or_else(|| fail("与 codeck-host 的连接已断开"));
    if let Some(err) = resp.get("error").and_then(Value::as_str) {
        fail(err);
    }
    let devices: Vec<PairedDevice> = serde_json::from_value(resp["devices"].clone())
        .unwrap_or_else(|e| fail(format!("解析设备列表失败：{e}")));
    if devices.is_empty() {
        println!("没有已配对的设备");
    } else {
        for dev in &devices {
            println!("{}", control::format_device_line(dev));
        }
    }
    std::process::exit(0);
}

fn run_unpair(state_dir: PathBuf, id: String) -> ! {
    let mut stream = connect_control(&state_dir);
    send_request(&mut stream, &json!({ "cmd": "unpair", "id": id }));
    let mut reader = BufReader::new(stream);
    let resp = read_response(&mut reader).unwrap_or_else(|| fail("与 codeck-host 的连接已断开"));
    if resp.get("notFound").and_then(Value::as_bool) == Some(true) {
        eprintln!("没有这台设备：{id}");
        std::process::exit(1);
    }
    if let Some(err) = resp.get("error").and_then(Value::as_str) {
        fail(err);
    }
    if let Some(removed) = resp.get("removed") {
        let name = removed["name"].as_str().unwrap_or("");
        println!("已移除 {}", control::sanitize_display_name(name));
        std::process::exit(0);
    }
    fail("移除设备失败");
}

fn main() {
    let cmd = match parse_command(std::env::args().skip(1)) {
        Ok(Some(c)) => c,
        Ok(None) => {
            println!("{USAGE}");
            return;
        }
        Err(e) => {
            eprintln!("codeck-host: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    match cmd {
        Command::Pair { state_dir } => run_pair(resolve_state_dir(state_dir)),
        Command::Devices { state_dir } => run_devices(resolve_state_dir(state_dir)),
        Command::Unpair { id, state_dir } => run_unpair(resolve_state_dir(state_dir), id),
        Command::Daemon(args) => run_daemon(args),
    }
}

fn run_daemon(args: Args) {
    let state_dir = resolve_state_dir(args.state_dir);
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let ifaces = net::interfaces().unwrap_or_else(|e| fail(format!("读不了网络接口：{e}")));
    // The default listeners follow the network: none yet is not an error (started before the network is up).
    let follow_network = args.listen.is_empty();
    let subnets = if follow_network {
        net::listeners(&[], &ifaces).unwrap_or_default()
    } else {
        net::listeners(&args.listen, &ifaces).unwrap_or_else(|e| fail(e))
    };
    if subnets.is_empty() {
        tracing::warn!("no private IPv4 address on a LAN interface yet; waiting for the network");
    }
    for s in &subnets {
        let name = ifaces
            .iter()
            .find(|i| i.addr == s.addr)
            .map_or("?", |i| i.name.as_str());
        tracing::info!("interface {name}: {}/{}", s.addr, s.prefix_len());
        if !s.addr.is_private() && !s.addr.is_loopback() {
            tracing::warn!(
                "{} is neither a private nor a loopback address: peers in {}/{} can reach this Host (TLS + pairing auth applies, but the interface is not private)",
                s.addr,
                s.addr,
                s.prefix_len()
            );
        }
    }
    let web_dir = args.web_dir.unwrap_or_else(default_web_dir);
    control::check_socket_path_len(&state_dir).unwrap_or_else(|e| fail(e));

    // Ensures state_dir is 0700, acquires daemon.lock, and loads devices.json.
    let sec = Arc::new(security::LanSecurity::open(state_dir.clone()).unwrap_or_else(|e| fail(e)));
    let settings_path = state_dir.join(codeck_host::settings::SETTINGS_FILE);
    let serving_security = if needs_tls_cert(follow_network, &subnets) {
        let cert = sec
            .ensure_cert()
            .unwrap_or_else(|e| fail(format!("准备 TLS 证书失败：{e}")));
        tracing::info!("tls certificate fingerprint: {}", cert.fingerprint);
        Some(Arc::clone(&sec))
    } else {
        None
    };

    let (events, sink) = server::event_channel();
    let host = Host::start(
        HostConfig {
            cache_dir: state_dir.join("cache"),
            settings_path,
            ccrpc_logs: Some(state_dir.join("logs/ccrpc")),
            images: Some(state_dir.join("images")),
            data_root: args.data_root,
            track_connect: true,
            keep_ls_watch: true,
            ..HostConfig::desktop()
        },
        sink,
    )
    .unwrap_or_else(|e| fail(format!("打不开索引：{e}")));
    tracing::info!(
        "state in {}, web in {}",
        state_dir.display(),
        web_dir.display()
    );
    let ctx = server::Ctx {
        host: host.clone(),
        events,
        allowed_origins: Arc::new(args.allow_origin),
        web_dir,
        gate: server::Gate::default(),
        security: None,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| fail(e));
    let run_res: Result<(), String> = runtime.block_on(async move {
        let port = args.port.unwrap_or(DEFAULT_PORT);
        let mut serving = server::Serving::new(
            port,
            Box::new(move |s, s_sec| server::router(ctx.with_security(s_sec), s)),
            serving_security,
        );
        for subnet in subnets {
            if let Err(e) = serving.start(subnet).await {
                serving.stop().await;
                return Err(format!("监听不了 {}:{port}：{e}", subnet.addr));
            }
        }

        // Bind control.sock only after initial listeners succeed so bind failures never leave a stale socket.
        let control_sock = match control::ControlSocket::bind(&state_dir) {
            Ok(s) => s,
            Err(e) => {
                serving.stop().await;
                return Err(e);
            }
        };
        let listening = serving.listening_handle();
        let control_ctx = control::ControlCtx::new(
            Arc::clone(&sec),
            port,
            Arc::new(move || listening.lock().clone()),
        );

        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(t) => t,
                Err(e) => {
                    drop(control_sock);
                    serving.stop().await;
                    return Err(e.to_string());
                }
            };

        {
            let shutdown = async {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            };
            tokio::pin!(shutdown);
            let control_loop = control_sock.serve(control_ctx);
            tokio::pin!(control_loop);

            if !follow_network {
                tokio::select! {
                    _ = &mut shutdown => {}
                    _ = &mut control_loop => {}
                }
            } else {
                let mut poll = tokio::time::interval(NETWORK_POLL);
                loop {
                    tokio::select! {
                        _ = &mut shutdown => break,
                        _ = &mut control_loop => break,
                        _ = poll.tick() => match net::interfaces() {
                            Ok(ifaces) => {
                                let wanted = net::listeners(&[], &ifaces).unwrap_or_default();
                                serving.follow(&wanted).await;
                            }
                            Err(e) => tracing::warn!("cannot read network interfaces: {e}"),
                        },
                    }
                }
            }
        }
        serving.stop().await;
        // Drop `control.sock` before releasing `daemon.lock` (held by `sec`) and before `process::exit`.
        drop(control_sock);
        drop(sec);
        Ok(())
    });
    host.shutdown();
    if let Err(e) = run_res {
        fail(e);
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &[&str]) -> Result<Option<Command>, String> {
        parse_command(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags_and_subcommands() {
        let Command::Daemon(a) = parse(&[
            "--port=9",
            "--listen",
            "127.0.0.1",
            "--listen",
            "192.168.1.2",
            "--allow-origin",
            "http://localhost:1420/",
            "--state-dir",
            "/s",
        ])
        .unwrap()
        .unwrap() else {
            panic!("expected Daemon");
        };
        assert_eq!(a.port, Some(9));
        assert_eq!(a.listen.len(), 2);
        assert_eq!(a.allow_origin, ["http://localhost:1420"]);
        assert_eq!(a.state_dir, Some("/s".into()));
        assert!(parse(&["--help"]).unwrap().is_none());
        assert!(parse(&["--listen", "::1"]).is_err());
        assert!(parse(&["--port"]).is_err());
        assert!(parse(&["--port", "0"]).is_err());
        assert!(parse(&["--port=0"]).is_err());
        assert!(parse(&["--bogus"]).is_err());

        assert_eq!(
            parse(&["pair", "--state-dir", "/d"]).unwrap(),
            Some(Command::Pair {
                state_dir: Some("/d".into())
            })
        );
        assert_eq!(
            parse(&["devices", "--state-dir=/d"]).unwrap(),
            Some(Command::Devices {
                state_dir: Some("/d".into())
            })
        );
        assert_eq!(
            parse(&["unpair", "abc123", "--state-dir", "/d"]).unwrap(),
            Some(Command::Unpair {
                id: "abc123".into(),
                state_dir: Some("/d".into())
            })
        );
        assert_eq!(
            parse(&["unpair", "--state-dir", "/d", "abc123"]).unwrap(),
            Some(Command::Unpair {
                id: "abc123".into(),
                state_dir: Some("/d".into())
            })
        );
        assert!(parse(&["unpair"]).is_err());
        assert!(parse(&["pair", "--bogus"]).is_err());
        assert!(parse(&["pair", "--help"]).unwrap().is_none());
    }

    #[test]
    fn cert_only_loaded_for_non_loopback_or_network_follow() {
        let lo = net::Subnet {
            addr: Ipv4Addr::LOCALHOST,
            mask: Ipv4Addr::new(255, 0, 0, 0),
        };
        let lan = net::Subnet {
            addr: Ipv4Addr::new(192, 168, 1, 20),
            mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        assert!(needs_tls_cert(true, &[]));
        assert!(!needs_tls_cert(false, &[lo]));
        assert!(needs_tls_cert(false, &[lo, lan]));
    }
}
