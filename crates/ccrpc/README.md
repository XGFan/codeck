# ccrpc

让 Host 本机的脚本用 JSONL 驱动跑在 ToH clay Pane 里的交互式 Claude Code：ccrpc 经 Holder 写 PTY，读 transcript / 进程文件 / 屏幕，并通过只含 hooks 的 ccrpc 插件得知审批、提问与轮次边界、代答审批与提问；插件不占上下文，也不改变 Claude Code 的表现。

## 前提

- macOS。
- Rust ≥ 1.91（edition 2024）。
- PATH 里有 `toh`（带 `toh ls --json` 与 Size 帧的版本，ToH ADR-0051），并已执行 `toh setup clay`。
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

从某个 Session 的一条提示词之前 Fork 出新的 Session（`claude --resume <sessionId> --fork-session` 后回退到那条提示词之前，再改名），做完发 `fork` 事件，之后同 `new` 一样连着它；不带 `--fork-at` 是复制整段，`--fork-restore both` 连代码一起恢复（确认页有覆盖警告时先以 `rewind` 请求问调用方）：

```sh
ccrpc new --cwd ~/proj --fork <sessionId> --fork-at <提示词 uuid> --fork-title "标题 (Fork)"
```

在 Pane 里按 `←` 把对话转入后台（Claude Code 的 agents 页面）后，对话由后台的 claude 以新的 sessionId 接着跑，Pane 里的 claude 只转发它的界面：ccrpc 跟过去（发 `session_changed`，进行中的一轮在那边结束），`ls` 也报后台的 Session；Pane 停在 agents 列表时要写入的命令报 `unrecognized`，回到对话（Esc）后照常。配置目录的后台 daemon 若是由 clay 外的 claude 拉起的，后台 claude 不会向插件报到，转入后台的对话跟不过去（已知限制，见 [docs/spec.md](docs/spec.md) 9 节）。

受控 Session 与人共用 Pane（[ADR-0001](docs/adr/0001-share-the-pane-and-resize-only-when-unreadable.md)）：它同时显示在调用方的界面与 ToH Client 里，两边都能输入，PTY 尺寸归人的 Client，ccrpc 的屏幕跟着它走。屏幕在这个尺寸下读不懂（窄屏下对话框折行、审批框的选项读不全、矮屏里输入框被挤出屏幕）时，ccrpc 把 PTY 调成 120×40 再读，不调回去，人的 Client 下次抢尺寸就换回它的。ToH 升级前就开着的 Pane 不能连，要关掉重开。

列出本机 clay 里运行中的 Claude Code（Holder ID、sessionId、status、cwd、插件是否已报到，制表符分隔；`--json` 输出数组，另带 Pane 是否隐藏、claude pid、版本与会话标题；`--json --watch` 先打一行，之后变了再打一行，stdin 关闭即退出，由文件与进程事件唤醒）：

```sh
ccrpc ls [--json [--watch]]
ccrpc hide | show <Holder ID>   # 在 Mac 上隐藏 / 显示这个 Pane（toh hide / toh show）
```

`new` 的 PTY 初始尺寸是 120×40（`--size COLSxROWS` 可改）；`new --hidden` 把 Pane 开在 ToH Mac Client 的隐藏窗口里（iOS 与 web 照常列出），没人显示它，尺寸就一直是这个；`--log FILE` 把版本、状态判定、识别决策与当时的整屏、hook 事件、响应与按键记进文件，排查用（粘贴的提示词会记下开头 60 个字）。

检查依赖与配置（toh、claude 版本、目录、TUI、ccrpc 插件、布局库、分析器），每行 `ok` / `warn` / `fail`：

```sh
ccrpc doctor
```

认不出的对话框可以交给分析器：设置 `CCRPC_ANALYZER_API_KEY`（Messages 兼容接口的 key），可选 `CCRPC_ANALYZER_BASE_URL`（缺省 `https://api.deepseek.com/anthropic`）、`CCRPC_ANALYZER_MODEL`（缺省 `deepseek-flash`）。分析得出的布局存进本机布局库（`CCRPC_LAYOUT_DIR`，缺省 `~/.local/share/ccrpc/layouts`）并立即使用；`ccrpc layouts list` 列出，`ccrpc layouts export <仓库目录>` 导出成内置布局。分析器会收到整屏文字与挂起工具的输入。每次分析（学会的与失败的）都在本机留一条记录（`CCRPC_ANALYSIS_DIR`，缺省 `~/.local/share/ccrpc/analyses`，只留最近 200 条）：当时的屏幕、送出的挂起工具输入、模型的回复与结果；`ccrpc analyzer records` 列出。`ccrpc analyzer test` 让分析器识别一个内置对话框，检验 key、接口与模型能不能用（成功时输出模型与用时，不存布局）。

Claude Code 配置目录缺省取 `CLAUDE_CONFIG_DIR`（再缺省为 `~/.claude`），可以在子命令前用 `--claude-config-dir DIR` 指定；`--toh-state-dir DIR`（或 `TOH_STATE_DIR`）只原样交给 `toh`。ccrpc 不读 ToH 状态目录里的文件：Holder 经 `toh ls --json` / `toh new --json` 得到，再按其中的 socket 连上。

一次交互。`>` 是调用方写入的一行，`<` 是 ccrpc 输出的一行：

```
< {"type":"ready","protocol":3,"holder_id":"m9523c50cf1","session_id":"6f0e2b7c-4d31-4f6a-9c2e-08b1d5a7c340","cwd":"/Users/me/proj","claude_version":"2.1.317","state":"idle","size":[120,40]}
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
| `prompt {text, images?}` / `prompt {parts}` | `idle` 时写提示词并回车，等 transcript 确认；`images` 为图片文件路径，先粘贴路径；`parts` 为按顺序排列的 `{"text": …}` 与 `{"image": 路径}`，逐项粘贴，图片落在文字中间（不能与 `text`、`images` 同给）；以 `!` 开头的照终端在 shell 模式里运行 |
| `steer {text, images?}` / `steer {parts}` | `busy` 时写入，在本轮工具调用之间送达（可带图片，同 `prompt`） |
| `follow_up {text, images?}` / `follow_up {parts}` | 进 ccrpc 队列，变成 `idle` 后依次按 `prompt` 发出（可带图片） |
| `abort` | 发 `Esc`，等状态离开 `busy` / `waiting` |
| `edit_queue` | `busy` 时同终端里在空输入框按 `↑`：把 Claude Code 队列里人输入的内容取回、清空输入框，返回取回的各条，供编辑后重发 |
| `answer_permission {request_id, decision, message?}` | 作答审批（经插件；后台 Subagent 的审批按屏幕选项键），`message` 只能配 `deny`，作为反馈交给模型 |
| `answer_question {request_id, answers}` | 经插件作答 AskUserQuestion（选项、自由文本，多选题可两者一起）；带 `chat: true` 时是对话框的 Chat about this（先聊聊），已答的题一并带上 |
| `slash {command}` | 输入 `/<command>` 后回车 |
| `set_model {model}` / `set_effort {level}` / `compact {instructions?}` / `clear` | 分别等于对应的 slash 命令（`/model`、`/effort` 也会改新 Session 的默认；中途切换的确认以 `choice` 出现） |
| `send_keys {keys}` | 依次写命名按键、`{"text":…}` 或 `{"bytes":…}` |
| `get_state` | 立即返回状态、挂起的请求、队列长度 |
| `get_messages {since?, before?, limit?, subagents?}` | 返回 Main Line 上的内容块与游标，可从最新往回分页、可带 Subagent Run 的块（后台线程增量读取） |
| `get_screen` | 立即返回屏幕文本与光标 |
| `get_tool_result {tool_use_id}` | 立即返回工具结果全文（落盘过的读文件） |
| `get_image {uuid, index?}` | 立即返回带图提示词里的一张图片（base64） |
| `answer_choice {request_id, option}` | 按选项文字作答 `choice` |
| `rewind {uuid}` | 在终端的 Rewind 选择器里找到 Main Line 上的这条提示词（或斜杠命令、`!` 命令），停在确认页，返回它写明的代码变化与可选的恢复项 |
| `answer_rewind {request_id, option \| cancel}` | 作答确认页：`both`（代码与对话）/ `conversation` / `code`，或取消；恢复了对话时返回放回输入框的提示词 |
| `set_mode {mode}` | 切权限模式：`manual` / `accept_edits` / `plan` / `auto` |
| `cancel_follow_up {index?}` | 撤销排队的 follow_up（不带 `index` 全部撤销） |
| `get_tasks` / `stop_task {task_id}` | 列出 / 停止后台 Subagent、shell、Monitor、Workflow Run；一轮结束、等消息的 Teammate 带 `idle: true` |
| `dismiss_feedback_draft` | 关掉模型起草的反馈卡片（终端里的 `0`）；发送只能在终端里做 |
| `exit {close_pane?, stop_tasks?}` | 退出 claude；`close_pane` 时连 Pane 一起结束；还有后台任务时只在带 `stop_tasks` 时停掉它们退出，否则报 `tasks_running` |

事件（没有 `id`）：

| 事件 | 说明 |
|---|---|
| `ready` | 连接后的第一条，含 Holder ID、sessionId、cwd、claude 版本与状态 |
| `state` | 状态变化，`waiting` 时带子类与 `waitingFor` |
| `block` | 一个新内容块（落盘的大工具结果带 `persisted`，Workflow Run 里的带 `run_id`） |
| `turn_end` | 一轮（一条提示词）结束：`prompt_id`、`trigger`、`last_text`、`error`、`aborted`、`background_tasks`、`duration_ms`、`usage`、`api_cost` |
| `permission_request` | 挂起的审批，`request_id` 是 `tool_use_id`，后台 Subagent 的带 `agent_id`；`options` 为审批框上的选项（含“总是允许”的全文），`layout_source` 说明是内置识别还是分析器学来的；computer use 的授权框（无编号菜单，按方向键作答）也作为审批发出 |
| `question` | 挂起的 AskUserQuestion |
| `dialog` | 其余对话框与启动界面 |
| `unrecognized` | 屏幕认不出，此时不按任何键 |
| `follow_up_failed` | 队列里的 `follow_up` 发不出去 |
| `branch` | Main Line 换到了另一个 Branch，`at_uuid` 之后已发的 block 作废 |
| `workflow` | Workflow Run 的整份状态（阶段、各 agent） |
| `choice` | 等待在一个非审批、非提问的菜单（如计划审批、启动时的目录信任框、`/add-dir` 的确认框，以及工具自己画的无编号菜单），用 `answer_choice` 作答；`layout_source` 区分内置布局与分析器学来的 |
| `layout_learned` / `analysis_failed` | 分析器学到新布局 / 没给出可用的布局 |
| `session_info` | 权限模式、模型、思考等级、上下文大小变了 |
| `progress` | busy 时屏幕上的进度行（动词、用时、token 数） |
| `feedback_draft` | 模型起草了产品反馈（输入框上方的卡片）出现 / 消失，带 SendFeedback 的标题与全文 |
| `session_changed` / `session_exit` | sessionId 换了 / Session 结束 |

响应两种：

```
{"type":"response","id":"2","command":"answer_permission","success":true,"data":{"state":"idle"}}
{"type":"response","id":"3","command":"abort","success":false,"error":{"code":"not_busy","message":"当前状态为 idle"}}
```

## 边界

- 必须装 ccrpc 插件（[ADR-0003](docs/adr/0003-require-ccrpc-plugin.md)）：只含 hooks，只在 clay Pane 里生效，不占上下文，不改变 Claude Code 的表现（人在 TUI 上看到、能做的与没有插件时一样）。不加 MCP、channel，不用 `-p`。
- 写只经 Holder 的 PTY 与插件的审批回复；读用 transcript、进程文件、屏幕与插件报来的事件；Claude Code 配置目录下的文件只读（插件由 `claude plugin` 命令安装），ToH 状态目录里的文件不读，只用 `toh` 命令与 Holder socket。
- 只服务本机 clay 里的 Session，调用方也只在 Host 本机；与人共用 Pane，读不懂屏幕时才调尺寸（[ADR-0001](docs/adr/0001-share-the-pane-and-resize-only-when-unreadable.md)）。
- 命令按到达顺序逐条执行；排队只有 `follow_up`。例外：命令等待期间，不写入、不等待的命令（`get_state`、`get_screen` 等）立即作答，`abort` 在那条命令按完最后一个键后立即执行；它们只可能先于正在执行的那条，绝不先于更早收到的命令（见 spec 5 节）。
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

`mock_model_turns`、`mock_model_session`、`mock_model_tools`、`mock_model_follow_up` 用测试里自带的本地 mock 模型（不消耗额度、不需要 key），覆盖插话在 Stop 之后自成一轮、后台任务的通知那一轮之后才发 follow_up、API 报错结束一轮、目录信任框、状态栏与进度事件、分页、`--log` 与 `exit`、审批的选项（`options`）、带 message 的拒绝与 AskUserQuestion；`mock_model_shared_pane` 覆盖与人共用 Pane（跟随人设的尺寸、读不出审批选项时调成 120×40）。设 `CCRPC_E2E_WIRE_DIR=目录` 时把每个 ccrpc 的收发原样录成 jsonl（CoDeck Connect 功能 E2E 的回放 fixture 由此而来）。

可选变量：`CCRPC_E2E_TUI=fullscreen`（用全屏 TUI 跑）、`CCRPC_E2E_BASE_URL`、`CCRPC_E2E_MODEL`、`CCRPC_E2E_TOH`（用哪个 `toh`；缺省用仓库同级目录的 `toh/target/debug/toh`，没有则用 PATH 里的）。

## 文档索引

- [docs/spec.md](docs/spec.md)：行为规格（命令行、状态、协议、写入、屏幕识别）。
- [docs/protocol.schema.json](docs/protocol.schema.json)：协议消息的 JSON Schema。
- [docs/probes.md](docs/probes.md)：Claude Code 的实测结论。
- [CONTEXT.md](CONTEXT.md)：术语。
- [docs/adr/](docs/adr/)：关键取舍。
- [TODO.md](TODO.md)：待办。
