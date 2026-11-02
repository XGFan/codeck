# ccrpc

让 Host 本机的脚本用 JSONL 驱动跑在 ToH clay Pane 里的交互式 Claude Code：ccrpc 只经 Holder 写 PTY、只读 transcript / 进程文件 / 屏幕，Claude Code 对此无感知。

## 前提

- macOS。
- Rust ≥ 1.91（edition 2024）。
- PATH 里有 `toh`，并已执行 `toh setup clay`。
- PATH 里有 `claude`。

## 构建

```sh
cargo build --release
```

产物是 `target/release/ccrpc`。

## 快速上手

在自己的项目目录里起一个受控 Session：

```sh
ccrpc new --cwd ~/proj
```

它经 `toh new` 在 clay 里启动 `claude`，随后进入 RPC 循环：stdin 每行一条命令，stdout 每行一条响应或事件，stderr 只写错误。stdin 关闭即断开，Session 不受影响。

接入已经在 clay Pane 里跑着的 Session：

```sh
ccrpc attach <Holder ID | sessionId>
```

ccrpc 不与人共用 Session：连上后把 PTY 设为 160×48（`--size COLSxROWS` 可改）；有人正在 ToH 上用这个 Pane（持有 PTY 尺寸）时 `attach` 报错，`--force` 强制连接；连着时有人接手，ccrpc 发 `taken_over` 并转为只读，调用方重新 `attach` 才能再写入。

列出本机 clay 里运行中的 Claude Code（Holder ID、sessionId、status、cwd，制表符分隔）：

```sh
ccrpc ls
```

ToH 状态目录与 Claude Code 配置目录缺省取 `TOH_STATE_DIR`、`CLAUDE_CONFIG_DIR`（再缺省为 `~/.local/state/toh`、`~/.claude`），也可以在子命令前用 `--toh-state-dir DIR`、`--claude-config-dir DIR` 指定。

一次交互。`>` 是调用方写入的一行，`<` 是 ccrpc 输出的一行：

```
< {"type":"ready","protocol":1,"holder_id":"m9523c50cf1","session_id":"6f0e2b7c-4d31-4f6a-9c2e-08b1d5a7c340","cwd":"/Users/me/proj","claude_version":"2.1.316","state":"idle","size":[160,48]}
> {"id":"1","type":"prompt","text":"给 src/util.rs 补一个单元测试"}
< {"type":"state","state":"busy"}
< {"type":"block","kind":"user","uuid":"2a1f…","index":0,"text":"给 src/util.rs 补一个单元测试"}
< {"type":"response","id":"1","command":"prompt","success":true,"data":{"uuid":"2a1f…"}}
< {"type":"block","kind":"text","uuid":"7c03…","index":0,"text":"先看一下这个文件。"}
< {"type":"block","kind":"tool_use","uuid":"9d4e…","index":0,"id":"toolu_01Read","name":"Read","input":{"file_path":"/Users/me/proj/src/util.rs"}}
< {"type":"block","kind":"tool_result","uuid":"b512…","index":0,"tool_use_id":"toolu_01Read","content":"1  pub fn add(a: i32, b: i32) -> i32 {…","is_error":false}
< {"type":"block","kind":"tool_use","uuid":"c8a7…","index":0,"id":"toolu_02Edit","name":"Edit","input":{"file_path":"/Users/me/proj/src/util.rs","old_string":"…","new_string":"…"}}
< {"type":"state","state":"waiting","waiting":{"kind":"permission","waiting_for":"permission prompt"}}
< {"type":"permission_request","request_id":"toolu_02Edit","tool_name":"Edit","input":{"file_path":"/Users/me/proj/src/util.rs","old_string":"…","new_string":"…"},"agent_id":null}
> {"id":"2","type":"answer_permission","request_id":"toolu_02Edit","decision":"allow"}
< {"type":"state","state":"busy"}
< {"type":"response","id":"2","command":"answer_permission","success":true,"data":{"state":"busy"}}
< {"type":"block","kind":"tool_result","uuid":"e019…","index":0,"tool_use_id":"toolu_02Edit","content":"The file … has been updated.","is_error":false}
< {"type":"block","kind":"text","uuid":"f3b2…","index":0,"text":"已补上单元测试。"}
< {"type":"state","state":"idle"}
< {"type":"turn_end","last_text":"已补上单元测试。"}
```

`prompt` 的响应在 transcript 里出现这条用户消息之后才回；命令执行期间事件照常发出，下一条命令等它返回。每个内容块在 transcript 里单独一行，各有自己的 `uuid`。

## 协议速览

每行一个 JSON 对象，`type` 区分种类。`id` 可选，字符串或数字，响应里原样带回。字段细节见 [docs/spec.md](docs/spec.md) 第 5 节。

命令（`data` 见 spec）：

| 命令 | 说明 |
|---|---|
| `prompt {text}` | `idle` 时写提示词并回车，等 transcript 确认 |
| `steer {text}` | `busy` 时写入，在本轮工具调用之间送达 |
| `follow_up {text}` | 进 ccrpc 队列，变成 `idle` 后依次发出 |
| `abort` | 发 `Esc`，等状态离开 `busy` / `waiting` |
| `answer_permission {request_id, decision, message?}` | 按屏幕选项键作答审批，`message` 只能配 `deny` |
| `answer_question {request_id, answers}` | 逐题作答 AskUserQuestion |
| `slash {command}` | 输入 `/<command>` 后回车 |
| `set_model {model}` / `compact {instructions?}` / `clear` | 分别等于对应的 slash 命令 |
| `send_keys {keys}` | 依次写命名按键、`{"text":…}` 或 `{"bytes":…}` |
| `get_state` | 立即返回状态、挂起的请求、队列长度 |
| `get_messages {since?}` | 立即返回 Main Line 上的内容块与 `cursor` |
| `get_screen` | 立即返回屏幕文本与光标 |
| `get_tool_result {tool_use_id}` | 立即返回工具结果全文（落盘过的读文件） |

事件（没有 `id`）：

| 事件 | 说明 |
|---|---|
| `ready` | 连接后的第一条，含 Holder ID、sessionId、cwd、claude 版本与状态 |
| `state` | 状态变化，`waiting` 时带子类与 `waitingFor` |
| `block` | 一个新内容块（落盘的大工具结果带 `persisted`，Workflow Run 里的带 `run_id`） |
| `turn_end` | 一轮结束，`last_text` 是本轮最后一段 assistant 文字 |
| `permission_request` | 挂起的审批，`request_id` 是 `tool_use_id` |
| `question` | 挂起的 AskUserQuestion |
| `dialog` | 其余对话框与启动界面 |
| `unrecognized` | 屏幕认不出，此时不按任何键 |
| `follow_up_failed` | 队列里的 `follow_up` 发不出去 |
| `branch` | Main Line 换到了另一个 Branch，`at_uuid` 之后已发的 block 作废 |
| `workflow` | Workflow Run 的整份状态（阶段、各 agent） |
| `taken_over` | 有人在 ToH 上接手了这个 Session，之后只读 |
| `session_changed` / `session_exit` | sessionId 换了 / Session 结束 |

响应两种：

```
{"type":"response","id":"2","command":"answer_permission","success":true,"data":{"state":"idle"}}
{"type":"response","id":"3","command":"abort","success":false,"error":{"code":"not_busy","message":"当前状态为 idle"}}
```

## 边界

- 对 Claude Code 透明：不加 hooks、MCP、channel、启动参数，不用 `-p`。
- 写只经 Holder 的 PTY；读只用 transcript、进程文件与屏幕；Claude Code 配置目录与 ToH 状态目录下的文件只读。
- 只服务本机 clay 里的 Session，调用方也只在 Host 本机；不与人共用 Session（[ADR-0001](docs/adr/0001-own-pty-size-and-yield-on-takeover.md)）。
- 命令按到达顺序逐条执行；排队只有 `follow_up`。
- 屏幕识别只认实测过的 Claude Code 2.1.316 布局，认不出就发 `unrecognized`，不猜、不按键。

## 测试

```sh
cargo test
```

单测回放 `tests/fixtures/` 里录制的 transcript 与屏幕文本。

端到端会启动真实 Claude Code，消耗模型额度，并使用隔离的 `TOH_STATE_DIR` 与 `CLAUDE_CONFIG_DIR`：

```sh
CCRPC_E2E_API_KEY=… cargo test --test e2e -- --ignored --nocapture --test-threads=1
```

可选变量：`CCRPC_E2E_BASE_URL`、`CCRPC_E2E_MODEL`、`CCRPC_E2E_TOH`（用哪个 `toh`；缺省用同级目录 `../toh/target/debug/toh`，没有则用 PATH 里的）。

## 文档索引

- [docs/spec.md](docs/spec.md)：行为规格（命令行、状态、协议、写入、屏幕识别）。
- [docs/probes.md](docs/probes.md)：Claude Code 的实测结论。
- [CONTEXT.md](CONTEXT.md)：术语。
- [docs/adr/](docs/adr/)：关键取舍。
- [TODO.md](TODO.md)：待办。
