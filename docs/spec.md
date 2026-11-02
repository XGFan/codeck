# ccrpc 规格

术语见 [CONTEXT.md](../CONTEXT.md)；下面引用的 Claude Code 行为都是实测结论，证据在 [probes.md](./probes.md)（Claude Code 2.1.316）。

## 1. 目标与边界

- 调用方只在 Host 本机；ccrpc 只服务本机 clay 里的 Session：`ccrpc new` 自己启动的，以及人手动在 clay Pane 里启动的。
- **对 Claude Code 透明**：不加 hooks、MCP、channel、启动参数，不用 `-p`。写只经 Holder 的 PTY；读只用 transcript、进程文件、屏幕。
- **对数据只读**：Claude Code 配置目录与 ToH 状态目录下的文件只用 `File::open` 读；连接 Holder socket 时作为普通附着端（`client: "ccrpc"`，不发 Resize，不改 PTY 尺寸）。
- MVP 不处理：人与调用方同时写入的冲突、同一 Session 连多个 ccrpc、远程调用、Workflow Run 内部结构、token 级流式、rewind 产生的 Branch（`block` 按 transcript 写入顺序发，不区分 Main Line 与废弃的 Branch）。

## 2. 命令行

```
ccrpc new [--cwd DIR] [-- claude 参数…]     # 经 toh new 在 clay 里启动 claude，然后进入 RPC 循环
ccrpc attach <Holder ID | sessionId>        # 接入已在运行的 Session，进入 RPC 循环
ccrpc ls                                    # 本机 clay 里运行中的 Claude Code：Holder ID、sessionId、status、cwd（制表符分隔）
ccrpc debug screen|keys|paste <Holder ID> … # 调试：打印屏幕、写按键、写 bracketed paste
```

- RPC 循环：stdin 每行一条命令，stdout 每行一条响应或事件；stderr 只写错误。stdin 关闭即断开（发 Detach），Session 不受影响。
- 目录：ToH 状态目录按 `--toh-state-dir` > `TOH_STATE_DIR` > `~/.local/state/toh`；Claude Code 配置目录按 `--claude-config-dir` > `CLAUDE_CONFIG_DIR` > `~/.claude`（两个选项写在子命令之前）。配置目录不是缺省值时，`new` 以 `env CLAUDE_CONFIG_DIR=… claude` 启动（`toh new` 只把少数变量带进 Pane）。`toh` 需在 PATH 里。claude 运行 20 秒仍没有进程文件时，在 stderr 提示可能是配置目录不对。

## 3. 定位

Holder ID → `<toh>/holders/<id>.json` 的 `child_pid`（Pane 的 shell）→ 它子孙进程里名为 `claude` 的进程 → `<cc>/sessions/<pid>.json`（`sessionId`、`cwd`）→ transcript `<cc>/projects/<cwd 中非字母数字换成 ->/<sessionId>.jsonl`（找不到时在 `projects/*/` 下按文件名找）。Subagent Run 的 transcript 是 `<sessionId>/subagents/agent-<id>.jsonl`，同名 `.meta.json` 的 `toolUseId` 关联到主线的 Agent 工具调用。屏幕尺寸取 Pane shell 的 tty（只读打开后 `TIOCGWINSZ`）。

- `attach <sessionId>`：在进程文件里找到 pid，再沿父进程链找到 `child_pid` 是其祖先的 Holder；不在 Holder 里的 Session 报错，不可控。
- **sessionId 会变**：`/clear` 后约 1.3s 进程文件的 `sessionId` 换新，ccrpc 切到新 transcript 并发 `session_changed`。
- 连接时（`ready` 之前）以及切换 Session 时，第一次查找就已存在的 transcript 当作历史读到文件末尾：不发 `block`，但记下挂起的工具调用和已出现的提示词（用于确认命令）。之后才出现的 transcript（新 Session 的第一条提示词、`/clear` 之后）按实时读。

## 4. 状态

| 状态 | 来源 |
|---|---|
| `starting` | 还没有进程文件：claude 未启动，或停在 onboarding / 目录信任等启动界面（这些界面期间没有进程文件） |
| `idle` / `busy` | 进程文件 `status` |
| `waiting` | `status = waiting`。子类：`waitingFor = "permission prompt"` → `permission`；`"input needed"` 且有挂起的 AskUserQuestion → `question`（tool_use 比状态晚约 120ms 落盘，最多等 1.5s 再判为 `dialog`）；其余 → `dialog` |
| `exited` | claude 进程消失或 Holder 断开 |

- 每 50ms 读一次进程文件与 transcript。
- **一轮**从 transcript 出现用户提示词、或状态变为 `busy` 开始。**`turn_end`**：状态为 `idle` 时读到 `system`/`turn_duration`（idle 比本轮最后内容早约 100ms；后台 Subagent 会让主线在 busy 期间先写一条 `turn_duration`，那条不算）；进入 idle 后 2s 仍没读到也发。下一轮在此之前就开始时，先补发上一轮的 `turn_end`。从 idle 打开又关掉的对话框不算一轮；一轮开了却既没 busy 也没结束标记（立即报错、在 busy 出现前中止）时，空闲 5 秒后结束。中止说明 `[Request interrupted by user…]` 与 `!` 命令的记录不算提示词。

## 5. 协议

每行一个 JSON 对象，`type` 区分种类。

- 命令：`{"id":"7","type":"prompt","text":"…"}`，`id` 可选，字符串或数字。
- 响应：`{"type":"response","id":"7","command":"prompt","success":true,"data":{…}}`；失败为 `"success":false,"error":{"code":"…","message":"…"}`。
- 事件：`{"type":"<事件名>",…}`，没有 `id`。
- 连接后先发 `ready`：`protocol`（1）、`holder_id`、`session_id`、`cwd`、`claude_version`、`state`（及 `waiting`）。
- **命令按到达顺序逐条执行**；需要等待的命令（如 `prompt` 等 transcript 确认）执行期间事件照常发出，但下一条命令要等它返回。

### 5.1 命令

| 命令 | 可用状态 | 做法 | 成功时 `data` |
|---|---|---|---|
| `prompt {text}` | idle 且上一轮已结束（上一轮只差结束标记时最多等 6s） | 先清空输入框（中止后被放回的提示词），bracketed paste 写正文，再发 `\r`（写入与回车前都确认屏幕上是输入框而不是对话框）；等 transcript 出现这条用户消息（≤30s）。以 `/` 或 `!` 开头的正文会被 Claude Code 当作斜杠命令或 shell 命令 | `uuid` |
| `steer {text}` | busy | 先清空输入框再同样写入；忙时输入会排队，在当前工具调用结束后送达（transcript `queue-operation`/`enqueue`，随后作为 `queued_command` 附件交给模型）。等 enqueue（≤10s） | `delivered_as`：`queued`；若这时本轮恰好结束、被当作新提示词，则为 `prompt` |
| `follow_up {text}` | 除 exited | 进 ccrpc 队列；在两条命令之间、`idle` 且上一轮已结束时按 `prompt` 依次发出（不会在别的命令等待期间发出）；失败发 `follow_up_failed` 事件 | `queued`（队列长度） |
| `abort` | busy / waiting | 发 `Esc`；等状态离开 busy / waiting（≤10s） | `state` |
| `answer_permission {request_id, decision, message?}` | waiting.permission | 识别审批框，按 `decision`（`allow` / `allow_always` / `deny`）对应选项的数字键；`message` 只能配 `deny`，拒绝后回到 idle 再作为 `prompt` 发出 | `state`；有 `message` 时另有 `message_uuid` |
| `answer_question {request_id, answers}` | waiting.question | `answers` 与题目一一对应：`{"options":[label…]}` 或 `{"other":"文本"}`；逐题等到屏幕显示该题再作答：单选按数字，多选按需切换勾选后 `→`（只有一题时移到 `Submit` 行回车），Other 按 `Type something` 的数字、输入文字、回车；多题最后在 Review 标签按 `Submit answers` | `state` |
| `slash {command}` | idle | 清空输入框，输入 `/<command>` 后回车（单行） | `{}` |
| `set_model {model}` / `compact {instructions?}` / `clear` | idle | 即 `slash` 的 `model X` / `compact …` / `clear` | `{}` |
| `send_keys {keys}` | 除 exited | 依次写入，每个间隔 80ms。元素：命名按键（`enter` `esc` `tab` `shift+tab` `backspace` `space` `up` `down` `left` `right` `home` `end` `pageup` `pagedown` `delete` `ctrl+c` `ctrl+d` `ctrl+u` `ctrl+l` `ctrl+o` `ctrl+r` `ctrl+t`）、`{"text":"…"}`（原样）、`{"bytes":"<base64>"}` | `{}` |
| `get_state` | 任意 | — | `state`、`waiting`、`request`（挂起的请求：`request_id`、`tool_name`、`input`）、`holder_id`、`session_id`、`cwd`、`follow_up_queue` |
| `get_messages {since?}` | 任意 | 当前 transcript 主文件的全部内容块，`since` 为上次返回的 `cursor` | `blocks`、`cursor` |
| `get_screen` | 任意 | — | `lines`（每行去尾部空白）、`cursor`（`[行, 列]`，0 起） |

错误码：`bad_request`（JSON 或字段不对，含答案与题目对不上）、`unknown_command`、`not_idle`、`not_busy`、`not_waiting`、`unknown_request`（`request_id` 不是当前挂起的请求）、`not_offered`（审批框没有“总是允许”）、`unsupported`（多选题的 Other）、`unrecognized_screen`、`input_not_empty`、`timeout`、`exited`、`io`。状态不满足时立即失败，不排队；排队只有 `follow_up`。

### 5.2 事件

| 事件 | 字段 |
|---|---|
| `ready` | 见上 |
| `state` | `state`；`waiting` 时另有 `waiting: {kind, waiting_for}` |
| `block` | `kind`（`user` / `text` / `thinking` / `tool_use` / `tool_result`）、`uuid`、`index`（消息内序号）；`text`；`tool_use` 的 `id`、`name`、`input`；`tool_result` 的 `tool_use_id`、`content`（文本，超过 100KB 截断）、`is_error`；忙时插话送达的用户内容 `queued: true`；来自 Subagent Run 时另有 `agent_id`、`parent_tool_use_id` |
| `turn_end` | `last_text`：本轮最后一段主线 assistant 文字（没有则为 null） |
| `permission_request` | `request_id`（= `tool_use_id`）、`tool_name`、`input`、`agent_id` |
| `question` | `request_id`、`questions`（AskUserQuestion 输入原样）、`agent_id` |
| `dialog` | `waiting_for`（进程文件原值；启动界面为 `startup`）、`lines`（屏幕）。启动界面每出现一个不同画面发一次 |
| `unrecognized` | `context`、`lines`；屏幕识别失败时发，不按任何键 |
| `follow_up_failed` | `text`、`error` |
| `session_changed` | `session_id`、`previous` |
| `session_exit` | — |

- 不发 slash 命令的本地记录（`<command-name>` 等）与 `isMeta` 条目。
- `question` 在进程文件为 `input needed` 且 transcript 里有未完成的 AskUserQuestion 时发。
- `permission_request` 按**屏幕上的审批框**认定对应的工具调用：在未完成的 tool_use（主线或 Subagent Run，排除 AskUserQuestion、Agent、Task）里，找命令（Bash）、文件名（Edit / Write / Read 等）、URL 或 pattern 出现在审批框正文里的那个；都没有可比对的内容时，只有唯一候选才认。3 秒内认不出就改发 `dialog`（交给 `send_keys`）。
- 状态变化、或已发出的请求对应的工具有了结果（连续两次审批可能合并成一段 waiting），都会重新认定并再发事件。已结束的请求不再参与匹配；仍有两个以上工具都能对上审批框时不认定（改发 `dialog`）。
- `answer_permission` 作答前重新确认屏幕上的审批框仍对应该 `request_id`，按键前再核对一次。

## 6. 写入

- 正文一次 bracketed paste（`ESC[200~ … ESC[201~`）写入，换行不会提交；再单独发 `\r`。4.5k 字符（含中文、换行）一次写入逐字正确。
- 写入前去掉控制字符（保留 `\n`、`\t`，`\r` 换成 `\n`），防止注入转义序列。
- 清空输入框：按输入框行数重复“`ctrl+u` + 退格”，再看屏幕确认；不用 `ctrl+c`（连按两次会退出 Claude Code）。

## 7. 屏幕识别

- 屏幕 = 一次 SnapshotReq 的结果按 PTY 尺寸用 `alacritty_terminal` 渲染。
- 审批框：`Do you want …?` 之后的编号菜单，按文字判断含义：`Yes` → allow；`Yes, …` 且含 `always allow` / `don't ask again` / `allow all`、不含 `auto mode` → allow_always；`No` 开头 → deny。必须同时有 allow 与 deny，否则不认。编号随场景变化，绝不按固定编号。
- AskUserQuestion：底部 `Enter to select … Esc to cancel` 提示行、上方 `☐` / `☒` 标签行（`←` 开头表示多题）、题目、编号选项（`[ ]` / `[✔]` 为多选）、`Type something`、`Chat about this`、多选题的 `Submit` 行；Review 标签为 `Review your answers` 与 `Submit answers`。
- 输入框：最下面两条整行 `─` 之间、以 `❯` 开头的内容。

## 8. 测试

- 单测：`tests/fixtures/` 的屏幕文本与 transcript（M0 录制、脱敏）回放：transcript 尾随、屏幕识别、协议解析。
- 端到端（`tests/e2e.rs`，`#[ignore]`）：独立 `TOH_STATE_DIR`（`/tmp` 下的短路径，unix socket 路径有约 104 字节上限；clay 启用、`[lan] bind = "127.0.0.1"`、`mdns = false`，模型相关变量经 `[backends.clay] env` 传给 Pane），去掉 `TOH_CONFIG` 与宿主终端变量；独立 `CLAUDE_CONFIG_DIR`（预置跳过 onboarding 与目录信任、`permissions.defaultMode = "default"`）；模型缺省 DeepSeek `deepseek-flash`（`https://api.deepseek.com/anthropic`），key 读 `CCRPC_E2E_API_KEY`。清理只杀记下的 Holder pid。

## 9. 已知限制

- 多选题的 Other（自由文本）未实测，返回 `unsupported`。
- rewind / 编辑重发产生的 Branch 不区分，`block` 与 `get_messages` 都按写入顺序。
- 屏幕布局按 2.1.316 识别；Claude Code 改了文字或布局时识别器会拒绝（`unrecognized`），需要更新 recognize.rs 与 fixture。
