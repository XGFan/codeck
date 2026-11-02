# CLAUDE.md

ccrpc：让 Host 本机的脚本以 JSONL（stdin / stdout）驱动跑在 ToH clay Pane 里的交互式 Claude Code，类似 Pi RPC。

- 术语以 [CONTEXT.md](CONTEXT.md) 为准；行为规格见 [docs/spec.md](docs/spec.md)；实测结论见 [docs/probes.md](docs/probes.md)；开发流程见 [docs/dev-process.md](docs/dev-process.md)；关键取舍见 [docs/adr/](docs/adr/)；待办只记 [TODO.md](TODO.md)。改动对外行为时同步 spec 与 README。

## 硬约束

- **对 Claude Code 透明**：不加 hooks、MCP、channel、启动参数，不用 `claude -p`。写只经 Holder 的 PTY，读只用 transcript、进程文件、屏幕。
- **只读**：Claude Code 配置目录与 ToH 状态目录下的文件只用 `File::open` 读，绝不写、移动、删除。
- 屏幕识别只认见过的布局，认不出就发 `unrecognized`，不猜、不按键。
- 端到端测试必须用独立的 `TOH_STATE_DIR` 与 `CLAUDE_CONFIG_DIR`，去掉 `TOH_CONFIG`、`KITTY_*`、`WEZTERM_*`、`KAKU_UNIX_SOCKET`、`ITERM_SESSION_ID`、`CLAUDECODE`、`TOH_HOLDER_ID`；测试 ToH 配置设 `[lan] bind = "127.0.0.1"`、`mdns = false`。清理只按记下的 PID / Holder ID。绝不碰主人真实的 clay Session。

## 常用命令

```sh
cargo build
cargo test                                             # 单测（fixture 回放）
cargo clippy --all-targets -- -D warnings
CCRPC_E2E_API_KEY=… cargo test --test e2e -- --ignored --test-threads=1   # 端到端（真实 Claude Code，消耗额度）
```

## 架构

单个 crate，`src/main.rs` 只做命令行分发，逻辑在库里：

- `rpc`：JSONL 编解码与命令分发。
- `locate`：Holder 元数据、进程树、进程文件、transcript 路径。
- `pty`：Holder unix socket 连接（toh-proto 帧）、写入（bracketed paste、按键）。
- `screen`：alacritty_terminal 还原屏幕文本；`recognize`：审批框与 AskUserQuestion 识别。
- `procfile`：进程文件轮询与状态映射。
- `transcript`：主线与 Subagent Run 的增量尾随（cv-core `parse` / `raw`），产出内容块。
- `session`：把以上合成状态机，执行命令、发事件。

依赖：cv-core（git，claude-viewer）、toh-proto（git，toh）。

Commit message 风格：`feat|fix|docs|test|refactor: <中文描述>`，常用「——」补充原因或做法。
