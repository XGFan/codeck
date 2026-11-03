# ccrpc 规格

术语见 [CONTEXT.md](../CONTEXT.md)；下面引用的 Claude Code 行为都是实测结论，证据在 [probes.md](./probes.md)（Claude Code 2.1.316；hooks 与插件为 2.1.317）。

## 1. 目标与边界

- 调用方只在 Host 本机；ccrpc 只服务本机 clay 里的 Session：`ccrpc new` 自己启动的，以及人手动在 clay Pane 里启动的。
- **必须装 ccrpc 插件**（[ADR-0003](./adr/0003-require-ccrpc-plugin.md)）：插件只含 hooks（不含 skills、agents、命令、MCP，不占上下文），只在 clay Pane 里生效（看 `TOH_HOLDER_ID`），hook 只在 PermissionRequest 时输出 ccrpc 给的决定，其余一律不写 stdout。没有插件的 Session 不能成为受控 Session。不加 MCP、channel，不用 `-p`。
- **不改变 Claude Code 的表现**：人在 TUI 上看到的界面与能做的操作与没有插件时一样；只有阻塞时界面照常显示的 hook 才会扣住等 ccrpc（见 5.3）。
- 写只经 Holder 的 PTY 与插件的 PermissionRequest 回复；读用 transcript、进程文件、屏幕与插件报来的 hook 事件。
- **对数据只读**：Claude Code 配置目录与 ToH 状态目录下的文件只用 `File::open` 读。`ccrpc plugin install` 经 `claude plugin` 命令安装插件，由 Claude Code 自己写配置。ccrpc 自己的状态目录（报到、hook socket）为 `CCRPC_STATE_DIR` > `$XDG_STATE_HOME/ccrpc` > `~/.local/state/ccrpc`。
- **不与人共用 Session**（[ADR-0001](./adr/0001-own-pty-size-and-yield-on-takeover.md)）：连接 Holder socket 时以 `client: "ccrpc"` 附着并订阅尺寸所有者通知，随后发 Resize 成为尺寸所有者。有人在用的 Session 不连，连着时被人接手就只读（见 2、5 节）。
- 不处理：同一 Session 连多个 ccrpc（第二个 `attach` 报 `已有 ccrpc 连着这个 Session`）、远程调用、token 级流式。

## 2. 命令行

```
ccrpc new [--cwd DIR] [--size COLSxROWS] [--co-display] [--log FILE] [-- claude 参数…]   # 经 toh new 在 clay 里启动 claude，然后进入 RPC 循环
ccrpc attach [--force] [--co-display] [--log FILE] [--size COLSxROWS] <Holder ID | sessionId>   # 接入已在运行的 Session，进入 RPC 循环
ccrpc ls [--json]                           # 本机 clay 里运行中的 Claude Code：Holder ID、sessionId、status、cwd、plugin（已报到）或 -（制表符分隔）；--json 为数组：holder_id、session_id、status、cwd、plugin、claude_pid、version、title
ccrpc doctor                                # 检查 toh、claude 版本、目录、TUI、ccrpc 插件、布局库、分析器（每行 ok / warn / fail）
ccrpc plugin install | uninstall            # 生成并安装 / 卸载 ccrpc 插件（用户级，已在运行的 claude 要重启才加载）
ccrpc plugin generate <目录>                 # 只生成插件（marketplace 在 <目录>，插件在 <目录>/ccrpc，供 claude --plugin-dir）
ccrpc layouts list                          # 布局库里的布局（id、builtin / learned、kind、title）
ccrpc layouts export <仓库目录>              # 把本机学到的布局写进仓库的 layouts/，重新构建后成为内置布局
ccrpc debug replay <transcript.jsonl>…      # 逐行回放 transcript：实时视图须与 get_messages 一致，打印每个 Branch
ccrpc debug screen|keys|paste <Holder ID> … # 调试：打印屏幕、写按键、写 bracketed paste
```

- RPC 循环：stdin 每行一条命令，stdout 每行一条响应或事件；stderr 只写错误。stdin 关闭即断开（发 Detach），Session 不受影响。
- PTY 尺寸：连上后 ccrpc 把 PTY 设为 `--size`（缺省 160×48）。尺寸归最后操作者，ToH 的 Client 只在把 Pane 切到前台或按键时才抢尺寸，所以尺寸所有者就是“谁在用”。
- 有人在用：`attach` 时尺寸所有者是另一个在线的 Client，就报错退出（`Session 正被 <client> 使用，加 --force 强制连接`）；`--force` 跳过。`new` 自己启动的 Pane 不检查。Holder 不报告尺寸所有者（旧版本）时放行，并在 stderr 提示。
- `debug` 子命令只看、只写，不改 PTY 尺寸。
- `--co-display`（共显）：不发 Resize、不判接手，VT 跟随 PTY 尺寸（谁设的都行），人在 Client 上的按键不算接手，`attach` 时有人在用也照连；写入照常（输入框照旧先清空）。`ready` 与 `get_state` 带 `co_display`。
- `--log FILE`：追加写日志（0600）：发出的事件（block 只记 kind 与 uuid）、状态判定、收到的命令、插件的 hook 事件、写入的按键与粘贴（粘贴只记长度与前 60 字，即提示词的开头也会进日志）。
- `ls --json` 的 `title`：transcript 里最后一条 `custom-title`，没有则最后一条 `ai-title`（transcript 取报到里的，没有报到时按 cwd 推算 `<projects>/<cwd 编码>/<sessionId>.jsonl`）。
- 插件：`new` 在插件未启用（用户 settings 的 `enabledPlugins` 里没有 `ccrpc@ccrpc`）且 claude 参数里没有 `--plugin-dir` 时报错；claude 的进程文件出现 5 秒仍没有报到也报错退出。`attach` 要求该 claude 已报到（见 3 节），否则报错并提示重启 claude。
- `ccrpc plugin install`：在 `$XDG_DATA_HOME/ccrpc/plugin`（缺省 `~/.local/share/ccrpc/plugin`）生成本地 marketplace `ccrpc` 与插件 `ccrpc`（hooks 调用当前 ccrpc 可执行文件的绝对路径），再经 `claude plugin marketplace add|update`、`claude plugin install|update` 装成用户级插件。换了 ccrpc 的位置要重新 install。
- 目录：ToH 状态目录按 `--toh-state-dir` > `TOH_STATE_DIR` > `~/.local/state/toh`；Claude Code 配置目录按 `--claude-config-dir` > `CLAUDE_CONFIG_DIR` > `~/.claude`（两个选项写在子命令之前）。配置目录不是缺省值时，`new` 以 `env CLAUDE_CONFIG_DIR=… claude` 启动（`toh new` 只把少数变量带进 Pane）。`toh` 需在 PATH 里。claude 运行 20 秒仍没有进程文件时，在 stderr 提示可能是配置目录不对。

## 3. 定位

Holder ID → `<toh>/holders/<id>.json` 的 `child_pid`（Pane 的 shell）→ 它子孙进程里名为 `claude` 的进程 → `<cc>/sessions/<pid>.json`（`status`、`cwd`）与插件的报到 `<ccrpc 状态目录>/sessions/<pid>.json`（`session_id`、`transcript_path`）。Subagent Run 的 transcript 是 `<sessionId>/subagents/agent-<id>.jsonl`，同名 `.meta.json` 的 `toolUseId` 关联到主线的 Agent 工具调用。Pane shell 的 tty（只读打开后 `TIOCGWINSZ`）给出连接时的 PTY 尺寸，也用于被接手后跟随尺寸、在旧 Holder 上判断接手。

- `attach <sessionId>`：在进程文件里找到 pid，再沿父进程链找到 `child_pid` 是其祖先的 Holder；不在 Holder 里的 Session 报错，不可控。
- **插件的 hook**：SessionStart、UserPromptSubmit、PreToolUse、PermissionRequest（超时 86400s）、PostToolUse、PostToolUseFailure、PermissionDenied、Stop、StopFailure、SubagentStart、SubagentStop。SubagentStart / SubagentStop 让 ccrpc 立即去找新的 Subagent Run 文件。插件协议版本（写在 hooks.json 的命令里：`<ccrpc> hook --protocol 2`，报到里的 `hook_protocol` 取自它，当前 2）与 ccrpc 不一致时不能 attach，要重新 `ccrpc plugin install` 并重启 claude。
- **报到**：插件的 SessionStart hook（`startup`、`resume`、`clear`、`compact`、`fork`）把 claude pid、`session_id`、`transcript_path`、hook 协议版本写进报到（ccrpc 自己的目录，0600）。`attach` 要求报到存在、hook 协议一致、`session_id` 与进程文件一致；只经 `/reload-plugins` 加载的插件不会报到。
- **hook 通道**：ccrpc 找到 claude 后监听 `<ccrpc 状态目录>/s/<claude pid>.sock`（先监听再读报到）；hook 按父进程（claude）pid 连接，连不上就无输出退出。
- **sessionId 会变**：`/clear`、`/resume` 等触发 SessionStart，ccrpc 按它给的 `session_id` 与 `transcript_path` 切到新 transcript 并发 `session_changed`。
- 连接时（`ready` 之前）以及切换 Session 时，已存在的 transcript 当作历史读到文件末尾：不发 `block`，但记下挂起的工具调用和已出现的提示词（用于确认命令）。之后才出现的 transcript（新 Session 的第一条提示词、`/clear` 之后）按实时读。

## 4. 状态

| 状态 | 来源 |
|---|---|
| `starting` | 还没有进程文件：claude 未启动，或停在 onboarding / 目录信任等启动界面（这些界面期间没有进程文件）。目录信任框作为 `choice` 发出（`layout: "trust_folder"`，选项 `Yes, I trust this folder` / `No, exit`），其余启动界面发 `dialog` |
| `idle` / `busy` | 进程文件 `status` |
| `waiting` | `status = waiting`。子类：`waitingFor = "permission prompt"` → `permission`；`"input needed"` 且插件报来了 AskUserQuestion 的 PermissionRequest → `question`（500ms 内没有则判为 `dialog`）；其余 → `dialog` |
| `exited` | claude 进程消失或 Holder 断开 |

- 由事件驱动：stdin 的新行、Holder 的帧（屏幕输出、尺寸所有者）、进程文件与 transcript 目录的文件变化都会立即触发一次检查；没有事件时每 250ms 兜底检查一次，命令等待期间每 50ms 一次。
- Subagent Run 的文件长度没变就不读；60 秒没有变化的改为每 5 秒查一次长度。
- **一轮**（见术语表）以插件为准：从 UserPromptSubmit 开始，到同一 `prompt_id` 的 Stop（完成）或 StopFailure（API 报错）结束；收到后等 transcript 写出本轮的 `turn_duration`（最多 2s）再发 `turn_end`，用量与用时才完整。中止（Esc、不带 message 的拒绝）不触发 Stop：一轮开着而状态回到 `idle` 1.5s 仍没有 Stop，按中止结束（`aborted: true`）。忙时插话的 UserPromptSubmit 带当前一轮的 `prompt_id`，不另开一轮；插话没在本轮里送达（本轮结束前没有工具调用可以夹带）时，它在 Stop 之后作为新的一轮运行、没有自己的 UserPromptSubmit：ccrpc 在上一轮结束时就开这一轮，由下一个 Stop 结束（`prompt_id` 取自 Stop）；这一轮若什么也没发生就回到 idle（插话其实已在上一轮送达），或者先来了别的 UserPromptSubmit，就不发 `turn_end`、直接丢掉；后台任务的结果通知是另一条提示词（`<task-notification>`），自成一轮（`trigger: "task_notification"`）；`!` 命令只有 Stop、没有 UserPromptSubmit，不算一轮。连接时已在进行的一轮由它的下一个 Stop 结束（`prompt_id` 为 null）。

## 5. 协议

每行一个 JSON 对象，`type` 区分种类。

- 命令：`{"id":"7","type":"prompt","text":"…"}`，`id` 可选，字符串或数字。
- 响应：`{"type":"response","id":"7","command":"prompt","success":true,"data":{…}}`；失败为 `"success":false,"error":{"code":"…","message":"…"}`。
- 事件：`{"type":"<事件名>",…}`，没有 `id`。
- 连接后先发 `ready`：`protocol`（2）、`holder_id`、`session_id`、`cwd`、`claude_version`、`state`（及 `waiting`）、`size`（`[列, 行]`）、`taken_over`、`co_display`、`mode`、`model`、`context_tokens`（同 `get_state`）、`warnings`（Claude Code 不是实测过的版本、Holder 不报尺寸所有者时各一句）。连接前就有的 Workflow Run 在 `ready` 之后各发一次当前状态。
- **被接手**：连着时尺寸所有者变成另一个 Client（旧 Holder 则是 PTY 尺寸变了），发 `taken_over`，清空 `follow_up` 队列；此后只有 `get_state`、`get_messages`、`get_screen`、`get_tool_result`、`get_tasks`、`cancel_follow_up`、`reclaim` 可用，其余命令返回 `taken_over`（命令执行中途被接手也在下一次写入前停下）。接手的人离开后用 `reclaim` 取回，或重新 `attach`。
- **命令按到达顺序逐条执行**；需要等待的命令（如 `prompt` 等 transcript 确认）执行期间事件照常发出，但下一条命令要等它返回。

### 5.1 命令

| 命令 | 可用状态 | 做法 | 成功时 `data` |
|---|---|---|---|
| `prompt {text, images?}` | idle 且上一轮已结束（上一轮只差结束时最多等 6s） | 先清空输入框（中止后被放回的提示词），bracketed paste 写正文，再发 `\r`（写入与回车前都确认屏幕上是输入框而不是对话框）；等 transcript 出现这条用户消息（≤30s）。以 `/` 或 `!` 开头的正文会被 Claude Code 当作斜杠命令或 shell 命令 ；`images` 为 Host 上的图片文件路径，正文之前逐个以 bracketed paste 写入路径、确认输入框出现 `[Image #n]` | `uuid`、`prompt_id`（插件看到的提交，没看到为 null） |
| `steer {text}` | busy | 先清空输入框再同样写入；忙时输入会排队，在当前工具调用结束后送达（transcript `queue-operation`/`enqueue`，随后作为 `queued_command` 附件交给模型）。等 enqueue（≤10s） | `delivered_as`：`queued`；若这时本轮恰好结束、被当作新提示词，则为 `prompt` |
| `follow_up {text}` | 除 exited | 进 ccrpc 队列；在两条命令之间、`idle` 且上一轮已结束时按 `prompt` 依次发出（不会在别的命令等待期间发出）；失败发 `follow_up_failed` 事件 | `queued`（队列长度） |
| `abort` | busy / waiting | 发 `Esc`；等状态离开 busy / waiting（≤10s） | `state` |
| `answer_permission {request_id, decision, message?}` | waiting.permission | 请求在插件里等着（主线）：`allow` 与 `deny` 经 hook 回决定——`deny` 不带 `message` 时中断本轮（同审批框的 No），带 `message` 时它作为反馈交给模型、本轮继续；`allow_always` 按审批框自己的“总是允许”选项（它加的规则如 `python3 *` 比 hook 能给的精确命令宽，见 probes.md），按键后放掉 hook，没有这一项则 `not_offered`。请求不在插件里等（后台 Subagent、hook 已超时）：识别审批框按对应选项的数字键，`message` 在拒绝、回到 idle 后作为 `prompt` 发出 | `state`；按键拒绝且有 `message` 时另有 `message_uuid` |
| `answer_question {request_id, answers}` | waiting.question | 只经插件：`answers` 与题目一一对应，`{"options":[label…]}`（多选以 `, ` 连接）或 `{"other":"文本"}`（单选、多选都可），作为 AskUserQuestion 的 `answers` 回给 hook；提问不在插件里等时返回 `unsupported` | `state` |
| `slash {command}` | idle | 清空输入框，输入 `/<command>` 后回车（单行） | `{}` |
| `set_model {model}` / `compact {instructions?}` / `clear` | idle | 即 `slash` 的 `model X` / `compact …` / `clear` | `{}` |
| `send_keys {keys}` | 除 exited | 依次写入，间隔见第 6 节。元素：命名按键（`enter` `esc` `tab` `shift+tab` `backspace` `space` `up` `down` `left` `right` `home` `end` `pageup` `pagedown` `delete` `ctrl+c` `ctrl+d` `ctrl+u` `ctrl+l` `ctrl+o` `ctrl+r` `ctrl+t`）、`{"text":"…"}`（原样）、`{"bytes":"<base64>"}` | `{}` |
| `get_state` | 任意 | — | `state`、`waiting`、`request`（挂起的请求：`request_id`、`tool_name`、`input`、`agent_id`；挂起的 choice 为它的 `choice` 事件）、`holder_id`、`session_id`、`cwd`、`follow_up_queue`、`follow_ups`、`taken_over`、`co_display`、`mode`（输入框下模式行最后一次看到的，没看到过取插件报来的 `permission_mode`）、`model`（主线最后一条 assistant 的模型）、`context_tokens`（它的 input + cache_read + cache_creation） |
| `get_messages {since?, before?, limit?, subagents?}` | 任意 | 当前 transcript 的 Main Line 内容块，与从头跟随事件得到的一致（`branch` 作废的块不在其中）。由单独的线程维护：第一次（连接或切换 Session 时就开始）读完整个文件，之后只读新追加的部分，读的期间事件照常发出。`since`：只要这个游标之后写入的块（上次的 `cursor`）；`before`：只要这个游标之前的（上一页的 `first`）；`limit`：范围内最新的这么多块；`subagents`：另给本页 Agent / Workflow 调用下的 Subagent Run 内容块。游标是不透明的数（文件偏移），Main Line 变了也有效 | `blocks`、`cursor`、`first`（本页第一个块的游标，没有块为 null；同一条 transcript 条目的块总在同一页，页可能比 `limit` 略多）、`more`（范围内还有更早的块）；`subagents` 时另有 `subagent_blocks` |
| `get_screen` | 任意 | — | `lines`（每行去尾部空白）、`cursor`（`[行, 列]`，0 起） |
| `cancel_follow_up {index?}` | 任意 | 撤销 follow_up 队列里第 `index` 条（0 起），不带则全部撤销 | `removed`、`follow_ups`（剩下的） |
| `reclaim {force?}` | 被接手后 | 接手的 Client 已断开（没有在线的尺寸所有者）时重新发 Resize、恢复写入并发 `reclaimed`；对方还在、或 Holder 不报尺寸所有者时返回 `taken_over`，`force` 跳过 | `size` |
| `set_mode {mode}` | idle / busy | `manual` / `accept_edits` / `plan` / `auto`：按 shift+tab，每次等输入框下的模式行变化，直到显示目标模式；转一圈没有则 `not_offered` | `mode` |
| `get_tasks` | 任意 | 后台任务：后台 Subagent、后台 shell、Workflow Run。以插件为准：PostToolUse 的工具结果开始它（`backgroundTaskId` / `agentId` / `taskId`），结果通知（UserPromptSubmit 的 `<task-notification>`）的 `status` 结束它，SubagentStop 先记为 `completed`，Stop 的 `background_tasks` 补上没见到开始的；连接前就有的从 transcript 读 | `tasks`：`id`、`kind`（`agent` / `shell` / `workflow`）、`tool_use_id`、`description`、`command`（后台 shell）、`status`（`running`、Claude Code 给的结束状态，或 ccrpc 停掉后的 `stopped`） |
| `exit {close_pane?, stop_tasks?}` | idle | 输入 `/exit`，等 claude 进程消失（≤15s），发 `session_exit`；还有后台任务时 Claude Code 先问，`stop_tasks` 时确认“退出并停掉”，否则按 Esc 返回 `not_idle`；`close_pane` 时再在 Pane 的 shell 里输入 `exit`，Pane 随之结束（Session 已 exited 时只做这一步） | `{}` |
| `stop_task {task_id}` | idle | 输入 `/tasks` 打开后台面板，按命令（后台 shell）或描述（Subagent、Workflow）找到以它开头的唯一一个运行中的行（没有、多于一个、或有同样的已结束行时都不按键，报 `unrecognized_screen`），一行一行移过去、确认光标在它上面后按 `x`；以面板为准确认它不再运行（≤5s），关面板；再等插件报来结果通知（≤2s，主线忙时要等当前工具结束），之前记为 `stopped` | `status`（通常 `killed`） |
| `answer_choice {request_id, option}` | waiting（有挂起的 choice）；目录信任框时为 starting | 等对话框出现满 500ms，另取一次 Snapshot 确认屏幕上仍是该布局，`option` 按选项文字（完全一致，否则唯一前缀）找到编号按下；目录信任框按方向键把光标移到该项、回车，等它关闭 | `state` |
| `get_tool_result {tool_use_id}` | 任意 | 工具结果全文：落盘过的读 `persisted` 文件，否则取 transcript 里的结果；超过 100KB 截断 | `content`、`persisted`（没落盘为 null） |

错误码：`bad_request`（JSON 或字段不对，含答案与题目对不上）、`unknown_command`、`not_idle`、`not_busy`、`not_waiting`、`unknown_request`（`request_id` 不是当前挂起的请求；`get_tool_result` 找不到该工具结果）、`taken_over`（Session 已被人接手）、`not_taken_over`（`reclaim` 时没被接手）、`not_offered`（没有“总是允许”）、`unsupported`（提问不在插件里等）、`unrecognized_screen`、`input_not_empty`、`timeout`、`exited`、`io`。状态不满足时立即失败，不排队；排队只有 `follow_up`。

### 5.2 事件

| 事件 | 字段 |
|---|---|
| `ready` | 见上 |
| `state` | `state`；`waiting` 时另有 `waiting: {kind, waiting_for}` |
| `block` | `kind`（`user` / `text` / `thinking` / `tool_use` / `tool_result`）、`uuid`、`index`（消息内序号）；`text`；`tool_use` 的 `id`、`name`、`input`；`tool_result` 的 `tool_use_id`、`content`（文本，超过 100KB 截断）、`is_error`，输出太大被存进 `tool-results/` 时 `content` 是 transcript 里的预览（模型看到的也只有它），另有 `persisted: {path, size}`；忙时插话送达的用户内容 `queued: true`；来自 Subagent Run 时另有 `agent_id`、`parent_tool_use_id`，属于 Workflow Run 时另有 `run_id`（`parent_tool_use_id` 为 Workflow 调用的 id） |
| `turn_end` | `prompt_id`：插件给的提示词 id（连接时已在进行的一轮为 null）；`trigger`：`prompt` / `task_notification`；`last_text`：Stop 的 `last_assistant_message`（StopFailure、中止为 null）；`error`：API 报错 `{kind, message}`（StopFailure 的 `error`，如 `rate_limit`、`server_error`、`unknown`），没有为 null；`aborted`：本轮被中止；`background_tasks`：Stop 时仍在运行的后台任务（Claude Code 原样：`id`、`type`、`status`、`description` 等），没有为 `[]`；`duration_ms`：`turn_duration` 的用时，没有则为 ccrpc 量的；`usage`：`input_tokens`、`output_tokens`、`cache_read_input_tokens`、`cache_creation_input_tokens`（本轮期间读到的主线与 Subagent Run 的 assistant 消息，每个 message id 计一次；连接时已在进行的一轮只算连接之后的部分）；`api_cost`：按 cv-core 价格表估算的美元，有模型没有价格时为 null |
| `permission_request` | `request_id`（= `tool_use_id`）、`tool_name`、`input`、`agent_id`（后台 Subagent 的请求） |
| `question` | `request_id`、`questions`（AskUserQuestion 输入原样）、`agent_id` |
| `dialog` | `waiting_for`（进程文件原值；启动界面为 `startup`）、`lines`（屏幕）。启动界面每出现一个不同画面发一次 |
| `unrecognized` | `context`、`lines`；屏幕识别失败时发，不按任何键 |
| `follow_up_failed` | `text`、`error` |
| `branch` | Main Line 换到了另一个 Branch（rewind、编辑重发）。`at_uuid` 是此前发出的 block 里最后一个仍在 Main Line 上的 `uuid`，在它之后发出的 block 都已作废；为 null 时此前发出的全部作废。在新 Branch 的第一个 block 之前发。只由人发的提示词触发：它沿父条目往上（跳过附件等不产生 block 的条目）碰到的不是最后发出的那个条目时，按 cv-core 的规则以它为 Main Line 重算；后台结果通知、teammate 消息、compact 边界、父条目还没写出的条目不算。原样重发同一条提示词且没有回复（中止说明不算回复）的不发 |
| `workflow` | Workflow Run 的整份状态：`run_id`、`tool_use_id`、`name`、`status`、`phases`（`index`、`title`、`detail`）、`agents`（`agent_id`、`label`、`phase_index`、`state`）。`workflows/<runId>.json` 或 `journal.jsonl` 内容有变化时发，内容不变不发 |
| `taken_over` | `client`：接手者的客户端标识（旧 Holder 为 null） |
| `reclaimed` | — |
| `choice` | `request_id`（`choice:<n>`）、`title`、`options`（`label` 屏幕上的全文、`meaning` `allow` / `allow_always` / `deny` / `other`、`description`）、`tool_use_id`（引起它的工具，如 ExitPlanMode；没有为 null）、`layout` |
| `layout_learned` | 分析器学到新布局并已存进布局库：`layout`、`kind`、`title`、`path` |
| `analysis_failed` | 分析器没给出可用的布局：`error`；结果与屏幕对不上时另有 `title` |
| `session_info` | `mode`、`model`、`context_tokens`（同 `get_state`）：其中任一项变了时发 |
| `progress` | busy 时屏幕上的进度行：`text`（去掉符号的整行）、`verb`、`elapsed_s`、`tokens`（没有为 null）；文字变了时发，至多每秒一次（动词变了立即发） |
| `session_changed` | `session_id`、`previous` |
| `session_exit` | — |

- 不发 slash 命令的本地记录（`<command-name>` 等）与 `isMeta` 条目。
- `question` 在进程文件为 `input needed` 且插件报来了 AskUserQuestion 时发。
- `permission_request` 来自插件的 PermissionRequest（它没有 `tool_use_id`，按紧挨在前的 PreToolUse 的工具名、输入与 agent 配对）。
- 状态变化、或已发出的请求结束（插件报来它的 PostToolUse / PostToolUseFailure / PermissionDenied，连续两次审批可能合并成一段 waiting），都会重新认定并再发事件；状态离开 `waiting` 不算结束（进程文件可能抖动），回到 `idle` 时主线的请求都结束（后台 Subagent 的请求留到它的结果）。经插件作答后 1.5 秒内不重新认定（进程文件与屏幕还停在正在关闭的对话框上）。
- 连接前就已在等的审批（插件没报过）：在 transcript 未完成的工具调用里按同样的规则认定屏幕上的审批框，按键作答。
- 按键作答的请求（后台 Subagent）按**屏幕上的审批框**认定：在这类请求里找命令（Bash）、文件名（Edit / Write / Read 等）、URL 或 pattern 出现在审批框正文里的那个；都没有可比对的内容时，只有唯一候选才认；1 秒内认不出就交给分析器或改发 `dialog`。作答前重新确认屏幕上的审批框仍对应该 `request_id`，按键前另取一次 Snapshot 再核对一次。

### 5.3 插件代答

- 插件的 PermissionRequest 等 ccrpc 一行回复。主线的审批与 AskUserQuestion：审批框与 hook 同时显示，ccrpc 扣住 hook 等调用方作答，回复决定后审批框收起（TUI 多一行 `Allowed / Denied by PermissionRequest hook`）。后台 Subagent 的请求（hook 阻塞期间审批框不显示）、ExitPlanMode（hook 只能拒绝）、被接手后的请求：ccrpc 立即回复“无决定”，界面照常出现，按屏幕作答。
- 放行：人先在 TUI 里作答（插件报来该工具的结果）、Session 回到 idle、被接手、ccrpc 断开时，ccrpc 放掉还扣着的 hook（无输出），审批框交还给人。PermissionRequest 的 hook 超时为 86400s；超时后审批框仍在，`answer_permission` 改为按键。

## 6. 写入

- 正文一次 bracketed paste（`ESC[200~ … ESC[201~`）写入，换行不会提交；等输入框里出现内容（≤2s，长正文显示为 `[Pasted text …]`）后再单独发 `\r`。4.5k 字符（含中文、换行）一次写入逐字正确。
- 按键之间至少隔 50ms，屏幕还没反应时最多等到 80ms。斜杠命令输入后等输入框出现该命令（≤1s）再回车，回车后等它离开输入框（≤2s）。
- 写入前去掉控制字符（保留 `\n`、`\t`，`\r` 换成 `\n`），防止注入转义序列。
- 写入前等屏幕上出现输入框（≤2s；刚启动时进程文件先说 idle、TUI 还没画好）。
- 清空输入框：按输入框行数重复“`ctrl+u` + 退格”，等屏幕上输入框变空（≤500ms，最多 3 次）；不用 `ctrl+c`（连按两次会退出 Claude Code）。

## 7. 屏幕识别

- 布局库（[ADR-0002](./adr/0002-ai-learned-layouts-take-effect-at-once.md)）：内置布局在仓库 `layouts/*.json`，编译时嵌入（`CCRPC_BUILTIN_LAYOUTS=0` 关闭，测试用）；学到的布局在 `CCRPC_LAYOUT_DIR`，缺省 `$XDG_DATA_HOME/ccrpc/layouts`（`~/.local/share/ccrpc/layouts`），每个布局一个文件，含当时的屏幕与 Claude Code 版本。布局 = 问题行（原文）+ 编号选项（编号、文字的稳定开头、含义）。用前做结构检查：屏幕上有该问题行，其后是 strict 编号菜单（编号不超过 9），编号与布局一致、每项文字以布局的文字开头；含义另有不依赖模型的字面检查：`No…` 只能是拒绝，`Yes…` 不能是拒绝（含 `auto mode` / `bypass` 的在审批类里只能是 `other`，在选择里不能是 allow_always），审批类恰好一个拒绝且在最后，其余文字的选项在审批类里只能是 `other`；`Do you want…` / `Run a dynamic workflow?` 不能是 choice；审批类还要对得上一个挂起的工具调用。审批等待中只有挂起的工具是 ExitPlanMode 时才发 choice。学到的布局加载时重算 id、在它自己的屏幕上复核。
- 分析器：内置识别与布局库都认不出、且等待超过 250ms 时，把整屏文字、`waitingFor` 与挂起工具（插件报来的请求，没有则为 transcript 里未完成的工具调用）的输入发给模型（`CCRPC_ANALYZER_API_KEY`，`CCRPC_ANALYZER_BASE_URL` 缺省 `https://api.deepseek.com/anthropic`，`CCRPC_ANALYZER_MODEL` 缺省 `deepseek-flash`；没有 key 时关闭）。内置识别器认得的屏幕不送分析；审批等待中认不出满 1 秒才分析；同一对话框只分析一次，每小时最多 10 次，同时只有一个分析；分析期间不降级为 `dialog`。结果在它自己的屏幕上通过结构检查才存进布局库，立即使用，发 `layout_learned`；否则发 `analysis_failed`，照旧 `dialog`。工具审批归为 `permission_request`，其余编号菜单发 `choice`。

- 屏幕 = 常驻的 `alacritty_terminal` VT：每收到 Holder 的 Snapshot 从头重建，之后的 Output 增量喂入；VT 尺寸跟 PTY 一致（被接手后按 tty 的 `TIOCGWINSZ` 跟随，变了就重新要一次 Snapshot）。
- 审批框：`Do you want …?` 之后的编号菜单；Workflow 的审批框问 `Run a dynamic workflow?`，菜单在脚本说明之后（说明里可能有编号的阶段，取其后最后一个从 1 开始的编号块），选项为 `Yes, run it` / `View raw script` / `No`。按文字判断含义：`Yes`（Workflow 为 `Yes, run it`）→ allow；`Yes, …` 且含 `always allow` / `don't ask again` / `allow all`、不含 `auto mode` → allow_always；`No` 开头 → deny。必须同时有 allow 与 deny，否则不认。编号随场景变化，绝不按固定编号。
- AskUserQuestion：底部 `Enter to select … Esc to cancel` 提示行、上方 `☐` / `☒` 标签行（`←` 开头表示多题）、题目、编号选项（`[ ]` / `[✔]` 为多选）、`Type something`、`Chat about this`、多选题的 `Submit` 行；Review 标签为 `Review your answers` 与 `Submit answers`。
- 输入框：最下面两条整行 `─` 之间、以 `❯` 开头的内容。

## 8. 测试

- 单测：`tests/fixtures/` 的屏幕文本与 transcript（M0 录制、脱敏）回放：transcript 尾随、屏幕识别、协议解析。
- 端到端（`tests/e2e.rs`，`#[ignore]`；`mock_model_turns` 用测试内置的本地 mock 模型，其余用真实模型）：ccrpc 插件为本次构建生成、经 `claude --plugin-dir` 加载（不安装），独立 `CCRPC_STATE_DIR`（`/tmp` 下）；独立 `TOH_STATE_DIR`（`/tmp` 下的短路径，unix socket 路径有约 104 字节上限；clay 启用、`[lan] bind = "127.0.0.1"`、`mdns = false`，模型相关变量经 `[backends.clay] env` 传给 Pane），去掉 `TOH_CONFIG` 与宿主终端变量；独立 `CLAUDE_CONFIG_DIR`（预置跳过 onboarding 与目录信任、`permissions.defaultMode = "default"`）；模型缺省 DeepSeek `deepseek-flash`（`https://api.deepseek.com/anthropic`），key 读 `CCRPC_E2E_API_KEY`。清理只杀记下的 Holder pid。

- 权限模式：输入框下的模式行，`manual mode on` / `accept edits on` / `plan mode on` / `auto mode on`，shift+tab 依次循环。
- 后台任务面板（`/tasks`）：标题 `Background`，下面的行带 `❯` 光标，提示行 `↑/↓ to select · Enter to view · x to stop · Esc to close`；只有一项时直接是详情（`… details`，提示行同样有 `x to stop`）。

## 9. 已知限制

- 屏幕布局按 2.1.316 识别；Claude Code 改了文字或布局时识别器会拒绝（`unrecognized`），需要更新 recognize.rs 与 fixture。
