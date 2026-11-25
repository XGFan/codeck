//! `codeck-host`: CoDeck's headless Host (ADR-0011). Indexes a data root like the desktop app and serves
//! the Host's `remote` methods and events over HTTP + WebSocket to the LAN, without authentication.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;

use codeck_host::lan::{DEFAULT_PORT, net, server};
use codeck_host::{Host, HostConfig};

/// How often the default listeners are checked against the machine's addresses.
const NETWORK_POLL: std::time::Duration = std::time::Duration::from_secs(5);

const USAGE: &str = "用法：codeck-host [--port <端口>] [--listen <IPv4>]… [--state-dir <目录>] [--data-root <目录>] [--web-dir <目录>] [--allow-origin <origin>]…

  --port          端口，默认 38471
  --listen        监听的 IPv4 地址，可重复；默认是所有已启用的局域网网卡（en*、eth*、wl*）的私网地址，随网络变化增减；其他网卡与回环地址只在这里指定时监听
  --state-dir     索引、日志、图片与 settings.json 所在的目录
  --data-root     Claude Code 的数据目录；默认依次取 settings.json、$CLAUDE_CONFIG_DIR、~/.claude
  --web-dir       /web/ 下提供的静态文件
  --allow-origin  允许跨域访问的网页来源，可重复";

#[derive(Debug, Default, PartialEq)]
struct Args {
    port: Option<u16>,
    listen: Vec<Ipv4Addr>,
    state_dir: Option<PathBuf>,
    data_root: Option<PathBuf>,
    web_dir: Option<PathBuf>,
    allow_origin: Vec<String>,
}

/// `None` for `--help`.
fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
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
                a.port = Some(v.parse().map_err(|_| format!("端口不对：{v}"))?);
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

/// `<exe dir>/web`; when that is missing, the repository's `dist-web` (a development build).
fn default_web_dir() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("web")))
        .unwrap_or_else(|| PathBuf::from("web"));
    let dev = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist-web"));
    if !beside.is_dir() && dev.is_dir() {
        return dev;
    }
    beside
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("codeck-host: {msg}");
    std::process::exit(1);
}

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("{USAGE}");
            return;
        }
        Err(e) => {
            eprintln!("codeck-host: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
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
                "{} is neither a private nor a loopback address: peers in {}/{} reach this Host without authentication",
                s.addr,
                s.addr,
                s.prefix_len()
            );
        }
    }
    let state_dir = args.state_dir.unwrap_or_else(default_state_dir);
    let web_dir = args.web_dir.unwrap_or_else(default_web_dir);
    let (events, sink) = server::event_channel();
    let host = Host::start(
        HostConfig {
            cache_dir: state_dir.join("cache"),
            settings_path: state_dir.join(codeck_host::settings::SETTINGS_FILE),
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
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| fail(e));
    runtime.block_on(async move {
        let port = args.port.unwrap_or(DEFAULT_PORT);
        let mut serving =
            server::Serving::new(port, Box::new(move |s| server::router(ctx.clone(), s)));
        for subnet in subnets {
            serving
                .start(subnet)
                .await
                .unwrap_or_else(|e| fail(format!("监听不了 {}:{port}：{e}", subnet.addr)));
        }
        let shutdown = shutdown_signal();
        if !follow_network {
            return shutdown.await;
        }
        tokio::pin!(shutdown);
        let mut poll = tokio::time::interval(NETWORK_POLL);
        loop {
            tokio::select! {
                _ = &mut shutdown => return,
                _ = poll.tick() => match net::interfaces() {
                    Ok(ifaces) => {
                        let wanted = net::listeners(&[], &ifaces).unwrap_or_default();
                        serving.follow(&wanted).await;
                    }
                    Err(e) => tracing::warn!("cannot read network interfaces: {e}"),
                },
            }
        }
    });
    host.shutdown();
    std::process::exit(0);
}

async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .unwrap_or_else(|e| fail(e));
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &[&str]) -> Result<Option<Args>, String> {
        parse_args(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags() {
        let a = parse(&[
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
        .unwrap();
        assert_eq!(a.port, Some(9));
        assert_eq!(a.listen.len(), 2);
        assert_eq!(a.allow_origin, ["http://localhost:1420"]);
        assert_eq!(a.state_dir, Some("/s".into()));
        assert!(parse(&["--help"]).unwrap().is_none());
        assert!(parse(&["--listen", "::1"]).is_err());
        assert!(parse(&["--port"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
