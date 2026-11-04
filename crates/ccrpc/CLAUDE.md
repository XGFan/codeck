# CLAUDE.md

ccrpc：让 Host 本机的脚本以 JSONL（stdin / stdout）驱动跑在 ToH clay Pane 里的交互式 Claude Code，类似 Pi RPC。

- 术语以 [CONTEXT.md](CONTEXT.md) 为准；行为规格见 [docs/spec.md](docs/spec.md)；实测结论见 [docs/probes.md](docs/probes.md)；开发流程见 [docs/dev-process.md](docs/dev-process.md)；关键取舍见 [docs/adr/](docs/adr/)；待办只记 [TODO.md](TODO.md)。改动对外行为时同步 spec 与 README。

## 硬约束

- **ccrpc 插件**（[ADR-0003](docs/adr/0003-require-ccrpc-plugin.md)）：受控 Session 必须装 ccrpc 插件。插件只含 hooks（不加 skills、agents、命令、MCP、channel），只在 clay Pane 里生效；hook 除 PermissionRequest 的决定外不向 stdout 写任何东西（会进上下文）。不用 `claude -p`。
- **不改变 Claude Code 的表现**：人在 TUI 上看到的界面与能做的操作与没有插件时一样。只扣住阻塞时界面照常显示的 hook（主线的审批与 AskUserQuestion）；后台 Subagent 的审批、ExitPlanMode 不扣，按屏幕作答。
- 写只经 Holder 的 PTY 与插件的 PermissionRequest 回复；读用 transcript、进程文件、屏幕与插件报来的 hook 事件。
- **只读**：Claude Code 配置目录与 ToH 状态目录下的文件只用 `File::open` 读，绝不写、移动、删除；插件经 `claude plugin` 命令安装。ccrpc 只写自己的状态目录（`CCRPC_STATE_DIR`）与数据目录。
- 屏幕识别只认布局库里的布局（实测的内置布局，或分析器分析得出、存在本机的布局，见 [ADR-0002](docs/adr/0002-ai-learned-layouts-take-effect-at-once.md)）；认不出、分析不出或过不了结构检查就发 `unrecognized`，不猜、不按键。
- 端到端测试必须用独立的 `TOH_STATE_DIR`、`CLAUDE_CONFIG_DIR` 与 `CCRPC_STATE_DIR`，插件经 `--plugin-dir` 加载、不安装，去掉 `TOH_CONFIG`、`KITTY_*`、`WEZTERM_*`、`KAKU_UNIX_SOCKET`、`ITERM_SESSION_ID`、`CLAUDECODE`、`TOH_HOLDER_ID`；测试 ToH 配置设 `[lan] bind = "127.0.0.1"`、`mdns = false`。清理只按记下的 PID / Holder ID。绝不碰主人真实的 clay Session。

## 常用命令

```sh
cargo build
cargo test                                             # 单测（fixture 回放）
cargo clippy --all-targets -- -D warnings
CCRPC_E2E_API_KEY=… cargo test --test e2e -- --ignored --test-threads=1   # 端到端（真实 Claude Code，消耗额度）
cargo test --test e2e mock_ -- --ignored --test-threads=1                 # 端到端里用本地 mock 模型的用例（不需要 key）
```

## 架构

单个 crate，`src/main.rs` 只做命令行分发，逻辑在库里：

- `rpc`：JSONL 编解码与命令分发。
- `hook`：`ccrpc hook`（插件每个 hook 运行它：报到、把事件交给连着的 ccrpc、等 PermissionRequest 的回复）与 ccrpc 一侧的 hook socket。
- `history`：`get_messages` 的后台线程，增量维护 Main Line 与 Subagent Run 的内容块，分页。
- `log`：`--log FILE` 的日志（`clog!`）。
- `plugin`：生成与安装 ccrpc 插件（`ccrpc plugin install`）。
- `locate`：Holder 元数据、进程树、进程文件、transcript 路径。
- `pty`：Holder unix socket 连接（toh-proto 帧）、常驻 VT 与尺寸所有者、写入（bracketed paste、按键）。
- `screen`：alacritty_terminal 维护屏幕文本；`recognize`：审批框、AskUserQuestion、输入框、模式行、后台任务面板、目录信任框、进度行的识别。
- `wake`：事件驱动的唤醒（stdin、Holder 帧、notify 文件监听）。
- `layout`：布局库（内置 `layouts/*.json` 由 build.rs 嵌入，学到的在本机），结构检查；`analyze`：分析器，认不出的对话框交给模型得出布局。
- `procfile`：进程文件轮询与状态映射。
- `transcript`：主线、Subagent Run 与 Workflow Run 的增量尾随（同仓库 `crates/transcript` 的 `raw`、`assemble`；本 crate 有同名模块，引用库时写 `::transcript::`），产出内容块、Branch 与 Workflow 事件。
- `session`：把以上合成状态机，执行命令、发事件；轮次以插件的 UserPromptSubmit / Stop / StopFailure 为准，审批与提问经 PermissionRequest 代答。

依赖：transcript（同仓库 `crates/transcript`，解析库；不依赖 cv-core 与 SQLite）、toh-proto（git，toh；toh 是 ssh 依赖，根目录 `.cargo/config.toml` 让 cargo 用 git 命令行拉取。要与本地 toh 联调时在自己的 `.cargo/config.toml` 里加 `[patch]`，不要提交）。

Commit message 风格：`feat|fix|docs|test|refactor: <中文描述>`，常用「——」补充原因或做法。
