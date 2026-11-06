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

/// SIGTERM came: the engine ends as when stdin closes.
static TERMINATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_sigterm(_: libc::c_int) {
    TERMINATED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// SIGTERM was received (CoDeck's Disconnect fallback): the run loop detaches from the Holder and
/// exits once the command in progress is done, never halfway through its keys.
pub(super) fn terminated() -> bool {
    TERMINATED.load(std::sync::atomic::Ordering::SeqCst)
}

fn run_engine(e: Engine) -> Result<()> {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe; system calls it
    // interrupts are restarted (BSD `signal` semantics on macOS).
    let handler: extern "C" fn(libc::c_int) = on_sigterm;
    unsafe { libc::signal(libc::SIGTERM, handler as *const () as libc::sighandler_t) };
    let lines = stdin_lines(e.wake.clone());
    e.run(lines)
}

/// `COLSxROWS`.
fn parse_size(s: &str) -> Result<(u16, u16)> {
    let (c, r) = s
        .split_once('x')
        .context("--size 的格式是 COLSxROWS，如 120x40")?;
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
pub fn resolve_target(roots: &Roots, target: &str) -> Result<Pane> {
    let panes = toh::list(roots)?;
    if let Some(p) = panes.iter().find(|p| p.id == target) {
        return Ok(p.clone());
    }
    // A sessionId: find its process via the Process Files, then the Holder above it.
    let rd =
        std::fs::read_dir(roots.sessions_dir()).context("读不到 Claude Code 的 sessions 目录")?;
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
        if let Some(pane) = locate::holder_of_pid(&table, &panes, pid) {
            return Ok(pane.clone());
        }
        bail!("Session {target} 不在 clay 的 Holder 里运行，无法控制");
    }
    bail!("找不到 {target}：既不是运行中的 Holder，也不是运行中的 Session")
}

const ATTACH_USAGE: &str = "用法：ccrpc attach [--log FILE] <Holder ID | sessionId>";

pub fn cmd_attach(roots: &Roots, args: &[String]) -> Result<()> {
    let mut target = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--log" => crate::log::open(Path::new(it.next().context("--log 需要文件")?))?,
            t if target.is_none() && !t.starts_with('-') => target = Some(t.to_string()),
            _ => bail!(ATTACH_USAGE),
        }
    }
    let Some(target) = target else {
        bail!(ATTACH_USAGE);
    };
    let pane = resolve_target(roots, &target)?;
    let table = locate::process_table()?;
    let Some(claude) = locate::find_claude_under(&table, pane.shell_pid) else {
        bail!("Holder {} 里没有运行中的 Claude Code", pane.id);
    };
    let state = hook::state_dir();
    let checkin = hook::read_checkin(&state, claude.pid);
    let pf = procfile::read_with_job(roots, claude.pid);
    if let Some(problem) = hook::checkin_problem(
        checkin.as_ref(),
        pf.as_ref().and_then(|(p, _)| p.session_id.as_deref()),
    ) {
        bail!(problem);
    }
    // A ccrpc connected to a conversation moved to the background listens on that claude's socket.
    let job = pf.and_then(|(_, j)| j.parked_job_id);
    let in_use = |pid| hook::socket_in_use(&state, pid);
    if in_use(claude.pid)
        || matches!(locate::moved(roots, &state, &pane.id, job.as_deref()),
            locate::Moved::In { pf, .. } if pf.pid.is_some_and(in_use))
    {
        bail!("已有 ccrpc 连着这个 Session");
    }
    run_engine(Engine::open(roots.clone(), &pane, Out::stdout())?)
}

pub fn cmd_new(roots: &Roots, args: &[String]) -> Result<()> {
    let (opts, claude_args) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let mut cwd = None;
    let mut hidden = false;
    let mut size = WORK_SIZE;
    let mut it = opts.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cwd" => cwd = Some(it.next().context("--cwd 需要目录")?.clone()),
            "--hidden" => hidden = true,
            "--log" => crate::log::open(Path::new(it.next().context("--log 需要文件")?))?,
            "--size" => size = parse_size(it.next().context("--size 需要 COLSxROWS")?)?,
            other => bail!("new：不认识的参数 {other}；claude 的参数放在 -- 之后"),
        }
    }
    // The plugin must be there for the Session to be controllable (ADR-0003).
    if !claude_args.iter().any(|a| a == "--plugin-dir") && !crate::plugin::enabled(roots) {
        bail!("ccrpc 插件未启用：先运行 ccrpc plugin install");
    }
    let mut toh_args = Vec::new();
    if let Some(d) = &cwd {
        toh_args.extend(["--cwd".to_string(), d.clone()]);
    }
    // The PTY starts at the size Claude Code is laid out for; whoever shows the Pane then sets it
    // (a hidden one keeps it: nobody shows it on the Mac).
    toh_args.extend(["--size".to_string(), format!("{}x{}", size.0, size.1)]);
    if hidden {
        toh_args.extend(["--window".to_string(), "hidden".to_string()]);
    }
    toh_args.push("--".into());
    // toh types the command into the Pane's login shell: `command` keeps the user's shell functions and
    // aliases named `claude` (wrappers that add arguments) out of it, so the arguments are exactly ours.
    toh_args.push("command".into());
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
        toh_args.push("env".into());
        toh_args.extend(env);
    }
    toh_args.push("claude".into());
    toh_args.extend(claude_args.iter().cloned());
    let pane = toh::new_pane(roots, &toh_args)?;
    run_engine(Engine::open(roots.clone(), &pane, Out::stdout())?)
}
