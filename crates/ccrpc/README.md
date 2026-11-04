# ccrpc

让 Host 本机的脚本用 JSONL 驱动跑在 ToH clay Pane 里的交互式 Claude Code：ccrpc 经 Holder 写 PTY，读 transcript / 进程文件 / 屏幕，并通过只含 hooks 的 ccrpc 插件得知审批、提问与轮次边界、代答审批与提问；插件不占上下文，也不改变 Claude Code 的表现。

## 前提

- macOS。
- Rust ≥ 1.91（edition 2024）。
- PATH 里有 `toh`，并已执行 `toh setup clay`。
- PATH 里有 `claude`。
- 装好 ccrpc 插件（见下）。

## 构建

```sh
cargo build --release
```

产物是 `target/release/ccrpc`。

## 快速上手

先装 ccrpc 插件（用户级；之后启动的 claude 生效，已在运行的要重启才加载）：

```sh
ccrpc plugin install      # 卸载：ccrpc plugin uninstall
```

插件只含 hooks，只在 clay Pane 里生效：SessionStart 时在 ccrpc 的状态目录（`CCRPC_STATE_DIR`，缺省 `~/.local/state/ccrpc`）报到，其余事件交给连着这个 Session 的 ccrpc，没有 ccrpc 连着时什么也不做。hooks 调用的是安装时那个 ccrpc 可执行文件，换了位置要重新 install。

在自己的项目目录里起一个受控 Session：

```sh
ccrpc new --cwd ~/proj
```

它经 `toh new` 在 clay 里启动 `claude`，随后进入 RPC 循环：stdin 每行一条命令，stdout 每行一条响应或事件，stderr 只写错误。stdin 关闭即断开，Session 不受影响。

接入已经在 clay Pane 里跑着的 Session：

```sh
ccrpc attach <Holder ID | sessionId>
```

没有加载插件的 Session（装插件前就在跑的 claude）不能 attach：重启 claude（如 `claude --resume <sessionId>`）后再连。一个 Session 同时只能连一个 ccrpc。

ccrpc 不与人共用 Session：连上后把 PTY 设为 160×48（`--size COLSxROWS` 可改）；有人正在 ToH 上用这个 Pane（持有 PTY 尺寸）时 `attach` 报错，`--force` 强制连接；连着时有人接手，ccrpc 发 `taken_over` 并转为只读，调用方重新 `attach` 才能再写入。

列出本机 clay 里运行中的 Claude Code（Holder ID、sessionId、status、cwd、插件是否已报到，制表符分隔；`--json` 输出数组，另带 claude pid、版本与会话标题）：

```sh
ccrpc ls [--json]
```

`new` / `attach` 的 `--co-display` 让受控 Session 同时显示在调用方界面与 ToH Client 里：不改 PTY 尺寸、人在 Client 上按键不算接手；`--log FILE` 把版本、状态判定、识别决策与当时的整屏、hook 事件、响应与按键记进文件，排查用（粘贴的提示词会记下开头 60 个字）。

检查依赖与配置（toh、claude 版本、目录、TUI、ccrpc 插件、布局库、分析器），每行 `ok` / `warn` / `fail`：

```sh
ccrpc doctor
```

认不出的对话框可以交给分析器：设置 `CCRPC_ANALYZER_API_KEY`（Messages 兼容接口的 key），可选 `CCRPC_ANALYZER_BASE_URL`（缺省 `https://api.deepseek.com/anthropic`）、`CCRPC_ANALYZER_MODEL`（缺省 `deepseek-flash`）。分析得出的布局存进本机布局库（`CCRPC_LAYOUT_DIR`，缺省 `~/.local/share/ccrpc/layouts`）并立即使用；`ccrpc layouts list` 列出，`ccrpc layouts export <仓库目录>` 导出成内置布局。分析器会收到整屏文字与挂起工具的输入。

ToH 状态目录与 Claude Code 配置目录缺省取 `TOH_STATE_DIR`、`CLAUDE_CONFIG_DIR`（再缺省为 `~/.local/state/toh`、`~/.claude`），也可以在子命令前用 `--toh-state-dir DIR`、`--claude-config-dir DIR` 指定。

一次交互。`>` 是调用方写入的一行，`<` 是 ccrpc 输出的一行：

```
< {"type":"ready","protocol":2,"holder_id":"m9523c50cf1","session_id":"6f0e2b7c-4d31-4f6a-9c2e-08b1d5a7c340","cwd":"/Users/me/proj","claude_version":"2.1.317","state":"idle","size":[160,48]}
> {"id":"1","type":"prompt","text":"给 src/util.rs 补一个单元测试"}
< {"type":"state","state":"busy"}
< {"type":"block","kind":"user","uuid":"2a1f…","index":0,"text":"给 src/util.rs 补一个单元测试"}
< {"type":"response","id":"1","command":"prompt","success":true,"data":{"uuid":"2a1f…","prompt_id":"5d0c…"}}
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
< {"type":"turn_end","prompt_id":"5d0c…","trigger":"prompt","last_text":"已补上单元测试。","error":null,"aborted":false,"background_tasks":[],"duration_ms":8120,"usage":{…},"api_cost":0.012}
```

`prompt` 的响应在 transcript 里出现这条用户消息之后才回；命令执行期间事件照常发出，下一条命令等它返回。每个内容块在 transcript 里单独一行，各有自己的 `uuid`。

## 协议速览

每行一个 JSON 对象，`type` 区分种类。`id` 可选，字符串或数字，响应里原样带回。字段细节见 [docs/spec.md](docs/spec.md) 第 5 节。

命令（`data` 见 spec）：

| 命令 | 说明 |
|---|---|
| `prompt {text, images?}` | `idle` 时写提示词并回车，等 transcript 确认；`images` 为图片文件路径，先粘贴路径 |
| `steer {text}` | `busy` 时写入，在本轮工具调用之间送达 |
| `follow_up {text, images?}` | 进 ccrpc 队列，变成 `idle` 后依次按 `prompt` 发出（可带图片） |
| `abort` | 发 `Esc`，等状态离开 `busy` / `waiting` |
| `answer_permission {request_id, decision, message?}` | 作答审批（经插件；后台 Subagent 的审批按屏幕选项键），`message` 只能配 `deny`，作为反馈交给模型 |
| `answer_question {request_id, answers}` | 经插件作答 AskUserQuestion（选项或自由文本） |
| `slash {command}` | 输入 `/<command>` 后回车 |
| `set_model {model}` / `compact {instructions?}` / `clear` | 分别等于对应的 slash 命令 |
| `send_keys {keys}` | 依次写命名按键、`{"text":…}` 或 `{"bytes":…}` |
| `get_state` | 立即返回状态、挂起的请求、队列长度 |
| `get_messages {since?, before?, limit?, subagents?}` | 返回 Main Line 上的内容块与游标，可从最新往回分页、可带 Subagent Run 的块（后台线程增量读取） |
| `get_screen` | 立即返回屏幕文本与光标 |
| `get_tool_result {tool_use_id}` | 立即返回工具结果全文（落盘过的读文件） |
| `get_image {uuid, index?}` | 立即返回带图提示词里的一张图片（base64） |
| `answer_choice {request_id, option}` | 按选项文字作答 `choice` |
| `set_mode {mode}` | 切权限模式：`manual` / `accept_edits` / `plan` / `auto` |
| `cancel_follow_up {index?}` | 撤销排队的 follow_up（不带 `index` 全部撤销） |
| `get_tasks` / `stop_task {task_id}` | 列出 / 停止后台 Subagent、shell、Workflow Run |
| `reclaim {force?}` | 接手的人离开后取回控制 |
| `exit {close_pane?, stop_tasks?}` | 退出 claude；`close_pane` 时连 Pane 一起结束 |

事件（没有 `id`）：

| 事件 | 说明 |
|---|---|
| `ready` | 连接后的第一条，含 Holder ID、sessionId、cwd、claude 版本与状态 |
| `state` | 状态变化，`waiting` 时带子类与 `waitingFor` |
| `block` | 一个新内容块（落盘的大工具结果带 `persisted`，Workflow Run 里的带 `run_id`） |
| `turn_end` | 一轮（一条提示词）结束：`prompt_id`、`trigger`、`last_text`、`error`、`aborted`、`background_tasks`、`duration_ms`、`usage`、`api_cost` |
| `permission_request` | 挂起的审批，`request_id` 是 `tool_use_id`，后台 Subagent 的带 `agent_id`；`options` 为审批框上的选项（含“总是允许”的全文），`layout_source` 说明是内置识别还是分析器学来的 |
| `question` | 挂起的 AskUserQuestion |
| `dialog` | 其余对话框与启动界面 |
| `unrecognized` | 屏幕认不出，此时不按任何键 |
| `follow_up_failed` | 队列里的 `follow_up` 发不出去 |
| `branch` | Main Line 换到了另一个 Branch，`at_uuid` 之后已发的 block 作废 |
| `workflow` | Workflow Run 的整份状态（阶段、各 agent） |
| `taken_over` / `reclaimed` | 有人在 ToH 上接手了这个 Session，之后只读 / 已取回 |
| `choice` | 等待在一个非审批、非提问的菜单（如计划审批、启动时的目录信任框），用 `answer_choice` 作答；`layout_source` 区分内置布局与分析器学来的 |
| `layout_learned` / `analysis_failed` | 分析器学到新布局 / 没给出可用的布局 |
| `session_info` | 权限模式、模型、上下文大小变了 |
| `progress` | busy 时屏幕上的进度行（动词、用时、token 数） |
| `session_changed` / `session_exit` | sessionId 换了 / Session 结束 |

响应两种：

```
{"type":"response","id":"2","command":"answer_permission","success":true,"data":{"state":"idle"}}
{"type":"response","id":"3","command":"abort","success":false,"error":{"code":"not_busy","message":"当前状态为 idle"}}
```

## 边界

- 必须装 ccrpc 插件（[ADR-0003](docs/adr/0003-require-ccrpc-plugin.md)）：只含 hooks，只在 clay Pane 里生效，不占上下文，不改变 Claude Code 的表现（人在 TUI 上看到、能做的与没有插件时一样）。不加 MCP、channel，不用 `-p`。
- 写只经 Holder 的 PTY 与插件的审批回复；读用 transcript、进程文件、屏幕与插件报来的事件；Claude Code 配置目录与 ToH 状态目录下的文件只读（插件由 `claude plugin` 命令安装）。
- 只服务本机 clay 里的 Session，调用方也只在 Host 本机；不与人共用 Session（[ADR-0001](docs/adr/0001-own-pty-size-and-yield-on-takeover.md)）。
- 命令按到达顺序逐条执行；排队只有 `follow_up`。
- 屏幕识别只认布局库里的布局：实测的内置布局（Claude Code 2.1.316 / 2.1.317，默认与全屏 TUI），或分析器分析得出的布局（[ADR-0002](docs/adr/0002-ai-learned-layouts-take-effect-at-once.md)）；认不出、分析不出就发 `unrecognized` 或 `dialog`，不猜、不按键。

## 测试

```sh
cargo test
```

单测回放 `tests/fixtures/` 里录制的 transcript 与屏幕文本。

端到端会启动真实 Claude Code，消耗模型额度，并使用隔离的 `TOH_STATE_DIR`、`CLAUDE_CONFIG_DIR` 与 `CCRPC_STATE_DIR`；ccrpc 插件为本次构建生成，经 `claude --plugin-dir` 加载，不安装：

```sh
CCRPC_E2E_API_KEY=… cargo test --test e2e -- --ignored --nocapture --test-threads=1
```

`mock_model_turns`、`mock_model_session`、`mock_model_tools`、`mock_model_follow_up` 用测试里自带的本地 mock 模型（不消耗额度、不需要 key），覆盖插话在 Stop 之后自成一轮、后台任务的通知那一轮之后才发 follow_up、API 报错结束一轮、目录信任框、状态栏与进度事件、分页、`--log` 与 `exit`、审批的选项（`options`）、带 message 的拒绝与 AskUserQuestion。设 `CCRPC_E2E_WIRE_DIR=目录` 时把每个 ccrpc 的收发原样录成 jsonl（cc-gui 的回放 fixture 由此而来）。

可选变量：`CCRPC_E2E_TUI=fullscreen`（用全屏 TUI 跑）、`CCRPC_E2E_BASE_URL`、`CCRPC_E2E_MODEL`、`CCRPC_E2E_TOH`（用哪个 `toh`；缺省用仓库同级目录的 `toh/target/debug/toh`，没有则用 PATH 里的）。

## 文档索引

- [docs/spec.md](docs/spec.md)：行为规格（命令行、状态、协议、写入、屏幕识别）。
- [docs/protocol.schema.json](docs/protocol.schema.json)：协议消息的 JSON Schema。
- [docs/probes.md](docs/probes.md)：Claude Code 的实测结论。
- [CONTEXT.md](CONTEXT.md)：术语。
- [docs/adr/](docs/adr/)：关键取舍。
- [TODO.md](TODO.md)：待办。
