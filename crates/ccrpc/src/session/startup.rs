//! `ccrpc attach` / `ccrpc new`: their options, finding the Holder, starting the Pane through
//! `toh`, and running the engine on stdin.

use super::*;

fn stdin_lines(wake: Arc<Wake>) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
            wake.bump();
        }
        // stdin closed: wake the loop so it sees the disconnect.
        wake.bump();
    });
    rx
}

fn run_engine(e: Engine) -> Result<()> {
    let lines = stdin_lines(e.wake.clone());
    e.run(lines)
}

/// Options of `new` / `attach`.
pub struct Opts {
    /// PTY size ccrpc sets.
    pub size: (u16, u16),
    /// Connect even when someone else holds the PTY size.
    pub force: bool,
    /// Shown alongside a person's Client: no Resize, no takeover (`--co-display`).
    pub co_display: bool,
}

/// `COLSxROWS`.
fn parse_size(s: &str) -> Result<(u16, u16)> {
    let (c, r) = s
        .split_once('x')
        .context("--size 的格式是 COLSxROWS，如 160x48")?;
    let (c, r): (u16, u16) = (
        c.parse().context("--size 的列数不对")?,
        r.parse().context("--size 的行数不对")?,
    );
    if c < 40 || r < 10 || c > 1000 || r > 500 {
        bail!("--size 须在 40x10 到 1000x500 之间");
    }
    Ok((c, r))
}

/// `attach <Holder ID | sessionId>`.
pub fn resolve_target(roots: &Roots, target: &str) -> Result<String> {
    if locate::valid_holder_id(target) && roots.holder_socket(target).exists() {
        return Ok(target.to_string());
    }
    // A sessionId: find its process via the Process Files, then the Holder above it.
    let rd =
        std::fs::read_dir(roots.sessions_dir()).context("读不到 Claude Code 的 sessions 目录")?;
    let holders = locate::list_holders(roots);
    let table = locate::process_table()?;
    for e in rd.flatten() {
        let p: PathBuf = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let Ok(pf) = serde_json::from_slice::<ProcessFile>(&bytes) else {
            continue;
        };
        if pf.session_id.as_deref() != Some(target) {
            continue;
        }
        let Some(pid) = pf.pid else { continue };
        // A pid reused by another process is not this Session (procStart check).
        if !procfile::is_alive(&pf) {
            continue;
        }
        if let Some((id, _)) = locate::holder_of_pid(&table, &holders, pid) {
            return Ok(id.clone());
        }
        bail!("Session {target} 不在 clay 的 Holder 里运行，无法控制");
    }
    bail!("找不到 {target}：既不是运行中的 Holder，也不是运行中的 Session")
}

pub fn cmd_attach(roots: &Roots, args: &[String]) -> Result<()> {
    let mut opts = Opts {
        size: DEFAULT_SIZE,
        force: false,
        co_display: false,
    };
    let mut target = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--force" => opts.force = true,
            "--co-display" => opts.co_display = true,
            "--log" => crate::log::open(Path::new(it.next().context("--log 需要文件")?))?,
            "--size" => opts.size = parse_size(it.next().context("--size 需要 COLSxROWS")?)?,
            t if target.is_none() && !t.starts_with('-') => target = Some(t.to_string()),
            _ => bail!(
                "用法：ccrpc attach [--force] [--co-display] [--log FILE] [--size COLSxROWS] <Holder ID | sessionId>"
            ),
        }
    }
    let Some(target) = target else {
        bail!(
            "用法：ccrpc attach [--force] [--co-display] [--log FILE] [--size COLSxROWS] <Holder ID | sessionId>"
        );
    };
    let target = &target;
    let holder = resolve_target(roots, target)?;
    let meta = locate::read_holder(roots, &holder)?;
    let table = locate::process_table()?;
    let Some(claude) = locate::find_claude_under(&table, meta.child_pid) else {
        bail!("Holder {holder} 里没有运行中的 Claude Code");
    };
    let checkin = hook::read_checkin(&hook::state_dir(), claude.pid);
    let pf = procfile::read(roots, claude.pid);
    if let Some(problem) = hook::checkin_problem(
        checkin.as_ref(),
        pf.as_ref().and_then(|p| p.session_id.as_deref()),
    ) {
        bail!(problem);
    }
    if hook::socket_in_use(&hook::state_dir(), claude.pid) {
        bail!("已有 ccrpc 连着这个 Session");
    }
    run_engine(Engine::open(roots.clone(), &holder, Out::stdout(), &opts)?)
}

/// `toh` against ccrpc's ToH state directory, without the caller's terminal and session variables.
pub(crate) fn toh_command(roots: &Roots) -> std::process::Command {
    let mut cmd = std::process::Command::new("toh");
    cmd.env("TOH_STATE_DIR", &roots.toh_state);
    for k in [
        "TOH_CONFIG",
        "KITTY_WINDOW_ID",
        "KITTY_LISTEN_ON",
        "WEZTERM_PANE",
        "WEZTERM_UNIX_SOCKET",
        "KAKU_UNIX_SOCKET",
        "ITERM_SESSION_ID",
        "TOH_HOLDER_ID",
        "CLAUDECODE",
        "CCRPC_ANALYZER_API_KEY",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

pub fn cmd_new(roots: &Roots, args: &[String]) -> Result<()> {
    let (opts, claude_args) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let mut cwd = None;
    let mut hidden = false;
    // A Pane ccrpc just started is not in anyone's use, whoever shows it.
    let mut eng = Opts {
        size: DEFAULT_SIZE,
        force: true,
        co_display: false,
    };
    let mut it = opts.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cwd" => cwd = Some(it.next().context("--cwd 需要目录")?.clone()),
            "--co-display" => eng.co_display = true,
            "--hidden" => hidden = true,
            "--log" => crate::log::open(Path::new(it.next().context("--log 需要文件")?))?,
            "--size" => eng.size = parse_size(it.next().context("--size 需要 COLSxROWS")?)?,
            other => bail!("new：不认识的参数 {other}；claude 的参数放在 -- 之后"),
        }
    }
    // The plugin must be there for the Session to be controllable (ADR-0003).
    if !claude_args.iter().any(|a| a == "--plugin-dir") && !crate::plugin::enabled(roots) {
        bail!("ccrpc 插件未启用：先运行 ccrpc plugin install");
    }
    let mut cmd = toh_command(roots);
    cmd.arg("new");
    if let Some(d) = &cwd {
        cmd.args(["--cwd", d]);
    }
    // Hidden: nobody shows the Pane on the Mac, so its PTY keeps the size it starts with; start at ccrpc's size,
    // big enough to recognize dialogs (co-display then follows whoever shows it).
    if hidden {
        let (c, r) = eng.size;
        cmd.args(["--window", "hidden", "--size", &format!("{c}x{r}")]);
    }
    cmd.arg("--");
    // `toh new` passes only a few variables to the Pane: carry a non-default config dir explicitly
    // so claude writes its Process File where ccrpc reads.
    // So does a non-default ccrpc state directory, for the plugin's hooks.
    let mut env = Vec::new();
    if roots.claude_config != Roots::default_claude_config() {
        env.push(format!(
            "CLAUDE_CONFIG_DIR={}",
            roots.claude_config.display()
        ));
    }
    if let Some(d) = std::env::var_os("CCRPC_STATE_DIR").filter(|v| !v.is_empty()) {
        env.push(format!("CCRPC_STATE_DIR={}", PathBuf::from(d).display()));
    }
    if !env.is_empty() {
        cmd.arg("env").args(env);
    }
    cmd.arg("claude").args(claude_args);
    let out = cmd
        .output()
        .context("运行 toh new 失败（toh 在 PATH 里吗？）")?;
    if !out.status.success() {
        bail!(
            "toh new 失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let holder = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !locate::valid_holder_id(&holder) {
        bail!("toh new 没有返回 Holder ID");
    }
    run_engine(Engine::open(roots.clone(), &holder, Out::stdout(), &eng)?)
}
