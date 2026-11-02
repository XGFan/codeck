# ccrpc 规格

术语见 [CONTEXT.md](../CONTEXT.md)。标「待 M0」的条目由实测探针确定后改写为现状（结论记在 [probes.md](./probes.md)）。

## 1. 目标与边界

- 调用方只在 Host 本机；ccrpc 只服务本机 clay 里的 Session：`ccrpc new` 自己启动的，以及人手动在 clay Pane 里启动的。
- **对 Claude Code 透明**：不加 hooks、MCP、channel、启动参数，不用 `-p`。写只经 Holder 的 PTY；读只用 transcript、进程文件、屏幕。
- **对数据只读**：Claude Code 的配置目录只用 `File::open` 读；ToH 的状态目录只读 Holder 元数据，连接 Holder socket 时作为普通附着端（不发 Resize，不改 PTY 尺寸）。
- MVP 不处理：人与调用方同时写入的冲突、同一 Session 连多个 ccrpc、远程调用、Workflow Run 内部结构、token 级流式。

## 2. 命令行

```
ccrpc new [--cwd DIR] [-- claude 参数…]     # 经 toh new 在 clay 里启动 claude，然后进入 RPC 循环
ccrpc attach <Holder ID | sessionId>        # 接入已在运行的受控 Session，进入 RPC 循环
ccrpc ls                                    # 列出本机 clay 里正在运行的 Claude Code（Holder ID、sessionId、状态、cwd）
```

- RPC 循环：stdin 每行一条命令，stdout 每行一条响应或事件；stderr 只写日志。stdin 关闭即退出，Session 不受影响。
- 目录：ToH 状态目录按 `TOH_STATE_DIR`（缺省 `~/.local/state/toh`）；Claude Code 配置目录按 `CLAUDE_CONFIG_DIR`（缺省 `~/.claude`）。两者都可用 `--toh-state-dir` / `--claude-config-dir` 覆盖。

## 3. 定位

Holder ID → `<toh>/holders/<id>.json` 的 `child_pid`（shell）→ 它的子孙进程里的 `claude` → `<cc>/sessions/<pid>.json`（`sessionId`、`cwd`）→ transcript `<cc>/projects/<编码后的 cwd>/<sessionId>.jsonl`（找不到时按 sessionId 在 `projects/*/` 下查找）。Subagent Run 的 transcript 在 `<sessionId>/subagents/agent-<id>.jsonl`，关联靠同名 `.meta.json` 的 `toolUseId`。

- `attach <sessionId>`：反查进程文件得到 pid，再沿父进程链找到所在 Holder（祖先是某个 Holder 的 `child_pid`）。不在任何 Holder 里的 Session 不可控，报错。
- **sessionId 会变**：`/clear`、`/resume` 后进程文件里的 `sessionId` 变成新的。ccrpc 跟着切换 transcript，并发 `session_changed`。
- `new`：`toh new` 打印 Holder ID 后，等 shell 的子孙里出现 `claude` 与它的进程文件；期间状态为 `starting`。

## 4. 状态

| 状态 | 来源 |
|---|---|
| `starting` | 还没有 claude 进程或进程文件 |
| `idle` / `busy` | 进程文件 `status` |
| `waiting` | 进程文件 `status = waiting`；子类按 `waitingFor`：`permission prompt` → `permission`；`input needed` 且挂起的工具调用是 AskUserQuestion → `question`；其余 → `dialog` |
| `exited` | claude 进程消失（shell 回到提示符或 Holder 退出） |

进程文件的变化按轮询读取（间隔 ≤ 100ms，文件 mtime 与大小不变时不解析）。

## 5. 协议

每行一个 JSON 对象，`type` 区分种类。

- 命令：`{"id":"7","type":"prompt","text":"…"}`，`id` 是调用方给的任意字符串。
- 响应：`{"type":"response","id":"7","command":"prompt","success":true,"data":{…}}`；失败为 `"success":false,"error":{"code":"busy","message":"…"}`。
- 事件：`{"type":"<事件名>",…}`，没有 `id`。
- 连接建立后先发一个 `ready` 事件：`{"type":"ready","protocol":1,"holder_id","session_id","cwd","claude_version","state"}`。

### 5.1 命令

| 命令 | 可用状态 | 做法 | 何时回响应 |
|---|---|---|---|
| `prompt {text}` | idle | bracketed paste 写入正文，再发 `\r` | transcript 里出现这条用户消息，`data.uuid` 为它的 uuid（超时 30s） |
| `steer {text}` | busy | 同上（忙时输入会排队）；处理时机待 M0 | 写入完成 |
| `follow_up {text}` | 任意（除 exited） | 进 ccrpc 的队列，变成 idle 后按 `prompt` 依次发出 | 入队即回，`data.queued` 为队列长度 |
| `abort` | busy / waiting | 发 `Esc` | 状态离开 busy / waiting（超时 10s） |
| `answer_permission {request_id, decision}` | waiting.permission | `decision`：`allow` / `allow_always` / `deny`；按屏幕上的选项按键 | 状态离开这次 waiting |
| `answer_question {request_id, answers}` | waiting.question | `answers` 按题目顺序，每题 `{"options":[label…]}` 或 `{"other":"文本"}`；逐题导航作答 | 状态离开这次 waiting |
| `slash {command}` | idle | 输入 `/<command>` 后回车，如 `{"command":"model sonnet"}` | 写入完成 |
| `set_model {model}` / `compact {instructions?}` / `clear` | idle | 分别等于 `slash` 的 `model X`、`compact …`、`clear` | 写入完成 |
| `send_keys {keys}` | 任意（除 exited） | `keys` 为数组，元素是命名按键（`enter`、`esc`、`tab`、`shift+tab`、`up`、`down`、`left`、`right`、`backspace`、`space`、`ctrl+c`…）或 `{"text":"…"}`（原样写入）或 `{"bytes":"<base64>"}` | 写入完成 |
| `get_state` | 任意 | — | 立即：`state`、`waiting`（子类与 `waitingFor`）、`request`（挂起的请求）、`session_id`、`cwd`、`follow_up_queue` |
| `get_messages {since?}` | 任意 | Main Line 上的内容块，`since` 为上次返回的 `cursor` | 立即：`blocks`、`cursor` |
| `get_screen` | 任意 | — | 立即：`lines`（每行去掉尾部空白）、`cursor` |

状态不满足时立即失败（`busy`、`not_idle`、`not_waiting`、`exited`、`unknown_request`），不排队；排队只有 `follow_up`。未知命令为 `unknown_command`，JSON 不合法为 `bad_request`。

### 5.2 事件

| 事件 | 字段 |
|---|---|
| `ready` | 见上 |
| `state` | `state`，`waiting?`：`{kind, waiting_for}` |
| `block` | `kind`（`user` / `text` / `thinking` / `tool_use` / `tool_result`）、`uuid`、`index`（消息内序号）、按 kind 的内容（`text`；`tool_use` 的 `id`/`name`/`input`；`tool_result` 的 `tool_use_id`/`content`/`is_error`）；来自 Subagent Run 时另带 `agent_id`、`parent_tool_use_id` |
| `turn_end` | busy → idle 时发，`last_text` 为本轮最后一段 assistant 文字 |
| `permission_request` | `request_id`（= `tool_use_id`）、`tool_name`、`input`、`agent_id?` |
| `question` | `request_id`、`questions`（AskUserQuestion 输入原样） |
| `dialog` | `waiting_for`、`lines`（屏幕文本） |
| `unrecognized` | `context`（哪个命令或状态）、`lines`；识别屏幕失败时发，此时不按任何键，对应命令失败为 `unrecognized_screen` |
| `session_changed` | `session_id`（新）、`previous` |
| `session_exit` | — |

- `block` 只发 Main Line 上的新内容块，按 transcript 写入顺序；ccrpc 启动时不回放历史（要历史用 `get_messages`）。
- `permission_request` / `question` 在进程文件变为对应 waiting **且** transcript（主线或 Subagent Run）里找到那条还没有结果的 tool_use 后才发；两者先后不定。

## 6. 写入

- 正文用 bracketed paste（`ESC[200~ … ESC[201~`）整段写入，换行不会提交；再单独发 `\r`。分块大小与间隔待 M0。
- 写入前去掉正文里的控制字符（保留 `\n`、`\t`），防止注入转义序列。
- 按键序列之间留间隔（待 M0），让 TUI 处理完一步再发下一步。

## 7. 屏幕识别

- VT 用 `alacritty_terminal`，尺寸取 Holder 的 PTY 尺寸（Snapshot 前先从 Meta / 元数据得到；变化时跟随 Output 中的重绘）。
- 识别器只认它见过的布局（审批框的选项、AskUserQuestion 的题目与选项）；对不上就发 `unrecognized`，不猜。已验证的 Claude Code 版本记在 probes.md。

## 8. 测试

- 单测：回放 `tests/fixtures/` 里的进程文件快照、transcript 片段、屏幕文本（M0 录制，脱敏）。
- 端到端（`cargo test --test e2e -- --ignored`，按需手动跑）：独立 `TOH_STATE_DIR`（clay 启用，`[lan] bind = "127.0.0.1"`、`mdns = false`），去掉 `TOH_CONFIG` 与宿主终端变量；独立 `CLAUDE_CONFIG_DIR`（预置跳过 onboarding 与目录信任），`ANTHROPIC_BASE_URL=https://ag.test4x.com`、模型 `deepseek-v4.1-flash`，key 读 `CCRPC_E2E_API_KEY`。

## 9. 待定（MVP 内由实测决定）

- 忙时输入的处理时机（steer 是插进工具调用之间还是等本轮结束）。
- bracketed paste 的上限、分块与提交时机。
- 审批框与 AskUserQuestion（含多选、Other）的布局与按键。
- 进程文件 `waiting` 与 transcript 写出 tool_use 的先后。
- 首次启动的界面（目录信任、onboarding、API key 确认）在进程文件里的表现。
- `/model X`、`/compact`、`/clear` 的反应（是否弹界面、sessionId 是否变）。
