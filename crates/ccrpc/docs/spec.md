# ccrpc 规格

术语见 [CONTEXT.md](../CONTEXT.md)；下面引用的 Claude Code 行为都是实测结论，证据在 [probes.md](./probes.md)（Claude Code 2.1.316；hooks 与插件为 2.1.317）。

## 1. 目标与边界

- 调用方只在 Host 本机；ccrpc 只服务本机 clay 里的 Session：`ccrpc new` 自己启动的，以及人手动在 clay Pane 里启动的。
- **必须装 ccrpc 插件**（[ADR-0003](./adr/0003-require-ccrpc-plugin.md)）：插件只含 hooks（不含 skills、agents、命令、MCP，不占上下文），只在 clay Pane 里生效（看 `TOH_HOLDER_ID`），hook 只在 PermissionRequest 时输出 ccrpc 给的决定，其余一律不写 stdout。没有插件的 Session 不能成为受控 Session。不加 MCP、channel，不用 `-p`。
- **不改变 Claude Code 的表现**：人在 TUI 上看到的界面与能做的操作与没有插件时一样；只有阻塞时界面照常显示的 hook 才会扣住等 ccrpc（见 5.3）。
- 写只经 Holder 的 PTY 与插件的 PermissionRequest 回复；读用 transcript、进程文件、屏幕与插件报来的 hook 事件。
- **对数据只读**：Claude Code 配置目录下的文件只用 `File::open` 读。ToH 状态目录里的文件一律不读：Holder 经 `toh` 命令的 JSON 输出列出、新建，再按它给的 socket 连上（ToH ADR-0051）。`ccrpc plugin install` 经 `claude plugin` 命令安装插件，由 Claude Code 自己写配置。ccrpc 自己的状态目录（报到、hook socket）为 `CCRPC_STATE_DIR` > `$XDG_STATE_HOME/ccrpc` > `~/.local/state/ccrpc`。
- **与人共用 Pane**（[ADR-0001](./adr/0001-share-the-pane-and-resize-only-when-unreadable.md)）：连接 Holder socket 时以 `client: "ccrpc"` 附着并订阅 PTY 尺寸（Size 帧），屏幕跟随人的 Client 设的尺寸，两边都能输入。只有屏幕在这个尺寸下读不懂时，ccrpc 才把 PTY 调成 120×40（见 7 节）。
- 不处理：同一 Session 连多个 ccrpc（第二个 `attach` 报 `已有 ccrpc 连着这个 Session`）、远程调用、token 级流式。

## 2. 命令行

```
ccrpc new [--cwd DIR] [--size COLSxROWS] [--hidden] [--log FILE] [-- claude 参数…]   # 经 toh new 在 clay 里启动 claude，然后进入 RPC 循环
ccrpc new … --fork <sessionId> [--fork-at <uuid>] [--fork-restore conversation|both] [--fork-title 标题] [-- claude 参数…]   # 同上，启动的是那个 Session 的 Fork（见下）
ccrpc attach [--log FILE] <Holder ID | sessionId>   # 接入已在运行的 Session，进入 RPC 循环
ccrpc ls [--json [--watch]]                 # 本机 clay 里运行中的 Claude Code：Holder ID、sessionId、status、cwd、plugin（已报到）或 -（制表符分隔）；--json 为数组：holder_id、session_id、status、cwd、plugin、hidden、claude_pid、version、title；--watch 先打一行，之后变了再打一行
ccrpc hide | show <Holder ID>               # 经 toh hide / toh show 在 Mac 上隐藏 / 显示这个 clay Pane
ccrpc doctor                                # 检查 toh、claude 版本、目录、TUI、ccrpc 插件、布局库、分析器（每行 ok / warn / fail）
ccrpc analyzer test                         # 让环境变量设定的分析器识别一个内置对话框（Change effort level?），结果须过布局的结构检查且类型一致；成功时输出一行 JSON：model、elapsed_ms、kind、options，不存布局库；失败时 stderr 写原因、退出码 1
ccrpc analyzer records                      # 分析记录：输出一行 JSON {dir, records}，dir 是记录目录（不存在时建好），records 从新到旧，每条有 file、at、outcome（learned / failed）、kind、title、error、layout、waiting_for、model、elapsed_ms、claude_version（不含屏幕、输入与回复，看文件）
ccrpc plugin install | uninstall            # 生成并安装 / 卸载 ccrpc 插件（用户级，已在运行的 claude 要重启才加载）
ccrpc plugin generate <目录>                 # 只生成插件（marketplace 在 <目录>，插件在 <目录>/ccrpc，供 claude --plugin-dir）
ccrpc layouts list                          # 布局库里的布局（id、builtin / learned、kind、title）
ccrpc layouts export <仓库目录>              # 把本机学到的布局写进仓库的 layouts/，重新构建后成为内置布局
ccrpc debug replay <transcript.jsonl>…      # 逐行回放 transcript：实时视图须与 get_messages 一致，打印每个 Branch
ccrpc debug screen|keys|paste <Holder ID> … # 调试：打印屏幕、写按键、写 bracketed paste
```

- RPC 循环：stdin 每行一条命令，stdout 每行一条响应或事件；stderr 只写错误。stdin 关闭即断开（发 Detach），Session 不受影响。收到 SIGTERM 时同样断开，但等正在执行的命令做完（不会停在按键序列中间）；连上 Session 之前（attach / new 启动期间）收到则直接结束。
- PTY 尺寸：连上不改尺寸，VT 跟随 Holder 报来的 PTY 尺寸（谁设的都行，变了就重新要一次 Snapshot）；人在 Client 上用这个 Pane 时 ccrpc 照常写入（输入框照旧先清空）。屏幕读不懂时调成 120×40，之后不调回去，人的 Client 下次抢尺寸（切到前台、按键）就换回它的尺寸（见 7 节）。`ready` 与 `get_state` 的 `size` 是 PTY 此刻的尺寸。
- Holder 不报 PTY 尺寸（ToH 升级前就开着的 Pane）时 `attach` 报错退出：关掉 Pane 重开。
- `debug` 子命令只看、只写，不改 PTY 尺寸。
- `--log FILE`：追加写日志（0600）：开头一行 ccrpc 与 Claude Code 的版本、Holder、sessionId；发出的事件与响应（block 只记 kind 与 uuid，带屏幕的事件另记整屏）、状态判定、识别决策（审批框用了哪种布局、choice 的布局及其来源，并记当时的整屏）、收到的命令、插件的 hook 事件、写入的按键与粘贴（粘贴只记长度与前 60 字，即提示词的开头也会进日志）。
- `--size`：`new` 的 PTY 初始尺寸（缺省 120×40；ToH 缺省的 80×24 里反馈草稿卡片会把输入框挤出屏幕），有人显示这个 Pane 时改成那一端的尺寸。
- `--hidden`：Pane 开在 clay 的隐藏窗口里（`toh new --window hidden`，ToH ADR-0050），Mac Client 不显示它，iOS 与 web 照常列出；没人显示时尺寸就停在 `--size`。`ls --json` 的 `hidden` 即 Pane 在不在隐藏窗口里，不在 clay 里的 Pane（kitty 等）为 `null`。
- **Fork**（`new --fork <sessionId>`，CoDeck 的 ADR-0009）：在 claude 参数之后追加 `--resume <sessionId> --fork-session`，`ready` 之后接着把它做成 Fork，结果以 `fork` 事件报出；做完之前（含挂起确认时）收到的命令排在它之后执行，只有不写入的命令（`get_state` 等）、作答等着调用方的界面的 `answer_choice`、`send_keys`、这次回退的 `answer_rewind` 与 `exit` 立即执行（不必等前面排着的），格式不对的行立即报错；`abort` 不会插进 Fork 的步骤。步骤：等到插件报到（`source: "fork"`，得知新 Session 的 id，照常发 `session_changed`）且 idle、输入框出现（启动界面照常以 `choice` / `dialog` 发出，由调用方作答；60 秒内没到 idle 时失败，得知新 Session 的 id 之后有 `choice` 或请求等着调用方作答的时间不计）→ 有 `--fork-at` 时回退到这条提示词之前（同 `rewind` + `answer_rewind`，但提示词按 **Origin Session** 的 transcript 找：`--fork-session` 起来后、回退或 `/rename` 之前新 Session 不写文件；`--fork-restore` 缺省为 `conversation`，`both` 时确认页没有这一项（之后没有代码改动）就按 `conversation`；选了 `both` 而确认页带 `⚠` 警告时不按，以 `rewind` 事件挂起这次回退，`options` 只列 `both` 与 `conversation`，等调用方 `answer_rewind`：选项照常作答，`cancel` 按 `conversation` 作答；还没按键就被拒绝（`unknown_request`、`not_waiting`）时仍挂起、可再答；确认页在终端里被作答时以终端的结果为准，被取消（没写出 transcript）报 `cancelled`；挂起期间 claude 退出报 `exited`）→ 有 `--fork-title` 时 `/rename <标题>`（先清空输入框里放回的提示词，不再放回；标题是单行）→ 等新 Session 的 transcript 出现（≤10s）→ 发 `fork`。Origin Session 的 transcript 按 id 在 projects 目录下找（先找 cwd 对应的目录）；新 Session 的 transcript 开头复制自它，这些条目（按 uuid）同转入后台的复制部分一样不发 `block`、不确认命令。失败（找不到 Origin Session 或 `--fork-at` 的提示词、回退失败、claude 先退出等）时发 `fork` 的 `error`，排着的命令都报 `exited`，claude 还在就退出它（先离开回退的选择器，`/exit` 不行时按两次 ctrl+c），再关掉 Pane（在 shell 里输入 `exit`），然后 ccrpc 以退出码 1 结束；做完之前 stdin 关闭或收到 SIGTERM 时同样结束 claude 与 Pane（没做完的 Fork 不留下）；回退作答之后（新 Session 已写出文件）`/rename` 失败不算失败，`fork` 带 `title_error`。
- `ls --json` 的 `title`：transcript 里最后一条 `custom-title`，没有则最后一条 `ai-title`（transcript 取报到里的，没有报到时按 cwd 推算 `<projects>/<cwd 编码>/<sessionId>.jsonl`）。
- 转入后台的 Pane（见 3 节）：`ls` 的 `session_id`、`status`、`cwd`、`version`、`title` 与 `plugin` 取后台 claude 的进程文件与报到；`claude_pid` 仍是 Pane 里的 claude。后台 claude 还没起来或已被停掉时照旧取 Pane 里的 claude 的。
- `ls --json --watch`：先打一行全表，之后表变了再打一行，stdin 关闭即退出。由 `toh ls --json --watch`（Pane 来去、隐藏 / 显示）与进程文件目录、报到目录的文件事件唤醒；`ps` 只在 Pane 或进程文件增减、列出的 claude 进程不在了时重跑，标题从上次读到的位置往后读。另每 30 秒看一次，兜住没删进程文件就退出的 claude。
- 插件：`new` 在插件未启用（用户 settings 的 `enabledPlugins` 里没有 `ccrpc@ccrpc`）且 claude 参数里没有 `--plugin-dir` 时报错；claude 的进程文件出现 5 秒仍没有报到也报错退出。`attach` 要求该 claude 已报到（见 3 节），否则报错并提示重启 claude。
- `ccrpc plugin install`：在 `$XDG_DATA_HOME/ccrpc/plugin`（缺省 `~/.local/share/ccrpc/plugin`）生成本地 marketplace `ccrpc` 与插件 `ccrpc`（hooks 调用当前 ccrpc 可执行文件的绝对路径），再经 `claude plugin marketplace add|update`、`claude plugin install|update` 装成用户级插件。换了 ccrpc 的位置要重新 install。
- 目录：`--toh-state-dir` 或 `TOH_STATE_DIR` 只原样交给 `toh`（缺省由 toh 自己定）；Claude Code 配置目录按 `--claude-config-dir` > `CLAUDE_CONFIG_DIR` > `~/.claude`（两个选项写在子命令之前）。`new` 让 toh 在 Pane 的登录 shell 里敲入 `command claude …`，`command` 使 shell 里同名的包装函数、别名不起作用，claude 的参数只有 `--` 之后给的；配置目录不是缺省值时为 `command env CLAUDE_CONFIG_DIR=… claude …`（`toh new` 只把少数变量带进 Pane）。`toh` 需在 PATH 里。claude 运行 20 秒仍没有进程文件时，在 stderr 提示可能是配置目录不对。

## 3. 定位

Holder ID → `toh ls --json` 里它的 `shell_pid`（Pane 的 shell）与 `socket` → shell 子孙进程里名为 `claude` 的进程 → `<cc>/sessions/<pid>.json`（`status`、`cwd`）与插件的报到 `<ccrpc 状态目录>/sessions/<pid>.json`（`session_id`、`transcript_path`）。Subagent Run 的 transcript 是 `<sessionId>/subagents/agent-<id>.jsonl`，同名 `.meta.json` 的 `toolUseId` 关联到主线的 Agent 工具调用。PTY 尺寸来自 Holder 的 Size 帧（附着时最先一帧，之后每次改尺寸一帧）。

- `attach <sessionId>`：在进程文件里找到 pid，再沿父进程链找到 `child_pid` 是其祖先的 Holder；不在 Holder 里的 Session 报错，不可控。
- **插件的 hook**：SessionStart、UserPromptSubmit、PreToolUse、PermissionRequest（超时 86400s）、PostToolUse、PostToolUseFailure、PermissionDenied、Stop、StopFailure、SubagentStart、SubagentStop。SubagentStart / SubagentStop 让 ccrpc 立即去找新的 Subagent Run 文件。插件协议版本（写在 hooks.json 的命令里：`<ccrpc> hook --protocol 2`，报到里的 `hook_protocol` 取自它，当前 2）与 ccrpc 不一致时不能 attach，要重新 `ccrpc plugin install` 并重启 claude。
- **报到**：插件的 SessionStart hook（`startup`、`resume`、`clear`、`compact`、`fork`）把 claude pid、`session_id`、`transcript_path`、hook 协议版本写进报到（ccrpc 自己的目录，0600）。`attach` 要求报到存在、hook 协议一致、`session_id` 与进程文件一致；只经 `/reload-plugins` 加载的插件不会报到。
- **hook 通道**：ccrpc 找到 claude 后监听 `<ccrpc 状态目录>/s/<claude pid>.sock`（先监听再读报到）；hook 按父进程（claude）pid 连接，连不上就无输出退出。
- **sessionId 会变**：`/clear`、`/resume` 等触发 SessionStart，ccrpc 按它给的 `session_id` 与 `transcript_path` 切到新 transcript 并发 `session_changed`。`new` 的 `ready` 往往在插件报到之前（启动界面期间没有报到），`session_id` 为 null，得知后发 `session_changed`（`previous` 为 null）。
- **转入后台**（agents 页面，2.1.318，见 [probes.md](./probes.md)）：Pane 里的 claude（P）的进程文件出现 `parkedJobId` 时，对话已交给后台 claude：找 `kind: "bg"`、`jobId` 与之相同且仍在运行的进程文件得到后台 claude（B），备用进程（`spare: true`）不算；它的插件报到须与进程文件的 `session_id` 一致、`source` 不是 `startup`（备用进程被认领前自己的报到）。不看报到里的 Holder：一个配置目录只有一个 daemon，后台 claude 带着拉起 daemon 的那个 Pane 的 Holder，P 与 B 只靠 `parkedJobId` = `jobId` 对上。跟随的 B 每次读进程文件都核对它仍在运行这个 job（pid 被别的进程重用时不算）。之后 hook socket 改为 `s/<B>.sock`，状态读 B 的进程文件，transcript 换成报到里的（NEW），发 `session_changed`（`previous` 为原来的）；写入仍经 Holder 的 PTY（P 转发 B 的界面）。切换前先把旧 transcript 读完、旧 socket 上排着的 hook 事件处理完；P 的锁（`s/<P>.lock`）一直留着，B 停掉期间别的 ccrpc 也连不上这个 Pane。NEW 开头是复制来的 OLD 全部条目（idle 时转入要等下一条提示词才写出，也可能分几次写）：ccrpc 记下 OLD 的条目 uuid，复制来的这些（以及开头那段里没有 uuid 的条目）只用来补全状态，不发 `block`、不计入本轮用量、不算 `turn_duration`、不确认命令。转入当时就跟过去的，NEW 按实时读（复制部分之后的是 B 新写的，如回复进行中转入时 B 写完的回复）；attach 时才发现的，NEW 按历史读。转入时正在进行的一轮由 B 接着写完（没有 UserPromptSubmit），B 的下一个 Stop 结束它，`prompt_id` 仍是原来的；B 忙着这一轮时提交的输入（它自己的 `prompt_id`）算这一轮的插话，B 闲着时提交的提示词（人的、后台任务的通知）则是新的一轮，这一轮按中止结束；B 闲着满 5 秒（认领的备用进程起来前进程文件是 idle）而没有 Stop 则按中止结束。B 被停掉后仍跟着这个 job，再打开时（新的 pid，同一 Session）改连新 pid 的 socket，不发 `session_changed`；`parkedJobId` 消失则跟回 P（实测没有出现过）。P 退出仍是 Session 结束（`session_exit`），即使 B 还在 daemon 里运行。`attach` 时另检查 P 的锁与 B 的 hook socket 是否已被占用；`attach <NEW 的 sessionId>` 经 `parkedJobId` 与之相同的 P 找到 Pane。
- 连接时（`ready` 之前）以及切换 Session 时，已存在的 transcript 当作历史读到文件末尾：不发 `block`，但记下挂起的工具调用和已出现的提示词（用于确认命令）。之后才出现的 transcript（新 Session 的第一条提示词、`/clear` 之后）按实时读。

## 4. 状态

| 状态 | 来源 |
|---|---|
| `starting` | 还没有进程文件：claude 未启动，或停在 onboarding / 目录信任等启动界面（这些界面期间没有进程文件）。目录信任框作为 `choice` 发出（`layout: "trust_folder"`，选项 `Yes, I trust this folder` / `No, exit`），其余启动界面发 `dialog` |
| `idle` / `busy` | 进程文件 `status` |
| `waiting` | `status = waiting`。子类：`waitingFor = "permission prompt"` → `permission`；`"input needed"` 且插件报来了 AskUserQuestion 的 PermissionRequest → `question`；500ms 内没有时，transcript 主线有未出结果的 AskUserQuestion（提问出现时没有 ccrpc 连着）→ `question`（题目取自 transcript，按屏幕作答），否则 → `dialog`；其余 → `dialog` |
| `exited` | claude 进程消失或 Holder 断开 |

- 由事件驱动：stdin 的新行、Holder 的帧（屏幕输出、PTY 尺寸）、进程文件与 transcript 目录的文件变化都会立即触发一次检查；没有事件时每 250ms 兜底检查一次，命令等待期间每 50ms 一次。
- Subagent Run 的文件长度没变就不读；60 秒没有变化的改为每 5 秒查一次长度。
- **一轮**（见术语表）以插件为准：从 UserPromptSubmit 开始，到同一 `prompt_id` 的 Stop（完成）或 StopFailure（API 报错）结束；收到后等 transcript 写出本轮的 `turn_duration`（最多 2s）再发 `turn_end`，用量与用时才完整。中止（Esc、不带 message 的拒绝）不触发 Stop：一轮开着而状态回到 `idle` 1.5s 仍没有 Stop，按中止结束（`aborted: true`）。忙时插话的 UserPromptSubmit 带当前一轮的 `prompt_id`，不另开一轮；插话没在本轮里送达（本轮结束前没有工具调用可以夹带）时，它在 Stop 之后作为新的一轮运行、没有自己的 UserPromptSubmit：ccrpc 在上一轮结束时就开这一轮，由下一个 Stop 结束（`prompt_id` 取自 Stop）；两个 Stop 几乎同时到、都早于 transcript 写出这两轮，所以后一个 Stop 先搁着，前一轮读到它的 `turn_duration`（或等满 2s）才结束，块与用量各归各轮；这一轮若什么也没发生就回到 idle（插话其实已在上一轮送达），或者先来了别的 UserPromptSubmit，就不发 `turn_end`、直接丢掉；后台任务的结果通知是另一条提示词（`<task-notification>`），自成一轮（`trigger: "task_notification"`）；`!` 命令只有 Stop、没有 UserPromptSubmit，不算一轮。连接时已在进行的一轮由它的下一个 Stop 结束（`prompt_id` 为 null）。

## 5. 协议

每行一个 JSON 对象，`type` 区分种类。

- 命令：`{"id":"7","type":"prompt","text":"…"}`，`id` 可选，字符串或数字。
- 响应：`{"type":"response","id":"7","command":"prompt","success":true,"data":{…}}`；失败为 `"success":false,"error":{"code":"…","message":"…"}`。
- 事件：`{"type":"<事件名>",…}`，没有 `id`。
- 连接后先发 `ready`：`protocol`（3）、`holder_id`、`session_id`、`cwd`、`claude_version`、`state`（及 `waiting`）、`size`（PTY 此刻的 `[列, 行]`）、`mode`、`model`、`context_tokens`（同 `get_state`）、`warnings`（Claude Code 不是实测过的版本时一句）、`feedback_draft`（连接时屏幕上已有的反馈草稿卡片，同 `feedback_draft` 事件，没有为 null）。连接前就有的 Workflow Run 在 `ready` 之后各发一次当前状态。
- **命令按到达顺序逐条执行**；需要等待的命令（如 `prompt` 等 transcript 确认）执行期间事件照常发出，下一条命令一般要等它返回，例外两种：不写入、不等待的命令（`get_state`、`get_screen`、`get_tasks`、`get_tool_result`、`get_image`、`follow_up`、`cancel_follow_up`）立即作答，响应可能先于正在执行的那条；`abort` 在正在执行的命令按完最后一个键、只剩等待时（`prompt` 与 `steer` 等 transcript、`send_now` 等队列取走、`get_messages` 全程），且有一轮可中止（busy / waiting）时立即执行，那条命令照常等到自己的结果，还在按键序列中间时等到这一刻。这两种都只可能先于正在执行的那条，绝不先于更早收到的命令：前面已有命令在排队时，它们也排在后面。其余命令排在它后面按到达顺序执行。

### 5.1 命令

| 命令 | 可用状态 | 做法 | 成功时 `data` |
|---|---|---|---|
| `prompt {text, images?}` 或 `prompt {parts}` | idle 且上一轮已结束（上一轮只差结束时最多等 6s） | 正文第一个字符是 `!` 时作为 `!` 命令运行（与终端相同；须为一行、不带图片，否则 `bad_request`；`follow_up` 排队的同样在发出时运行）：清空输入框后粘贴 `!命令`（开头的 `!` 让输入框进 shell 模式），确认屏幕上是 shell 模式（输入框那行以 `!` 开头）且其中正是这条命令（不计空白），回车前再确认仍 idle，否则清空输入框、不回车；它不触发 UserPromptSubmit，回车前即开一轮、由它的 Stop 结束（`turn_end` 的 `prompt_id` 取 Stop 的）；等 transcript 出现它的 `<bash-input>`（≤10s，开始运行时即写入），返回其 `uuid`；没等到但命令已离开输入框时照常成功、`uuid` 为 null（不报超时，免得调用方重发再运行一次）；`prompt_id` 为 null。`steer` 与拒绝审批时带的 `message` 第一个字符是 `!` 时报 `bad_request`（粘贴会进 shell 模式）。先清空输入框（中止后被放回的提示词），bracketed paste 写正文，再发 `\r`（写入与回车前都确认屏幕上是输入框而不是对话框）；等 transcript 出现这条用户消息（≤30s）；回车前 Claude Code 恰好自己开始了一轮（后台任务的通知等），提示词会进它的队列（transcript 出现 enqueue），也算送达；enqueue 不带正文时（别的 Session 发来的消息也是这样）要再过 2 秒仍没有这条用户消息、且正文已离开输入框才算。以 `/` 或 `!` 开头的正文会被 Claude Code 当作斜杠命令或 shell 命令；斜杠命令以 transcript 里它的命令记录（`<command-name>` 的 user 条目，新版本地命令为 `system` / `local_command` 条目，还原成 `/name args` 比对，不计空白；插件的命令只写了名字时 Claude Code 记下带插件名的全名，如 `/autopilot x` 记为 `/oh-my-claudecode:autopilot x`，名字是全名冒号后的部分、参数相同也算）确认，`uuid` 为该记录的 uuid；打开面板、不留记录的命令（如 `/cost`、`/status`）要等满 30 秒才超时，这期间会写入的命令也在排队，应改用 `slash`；`images` 为 Host 上的图片文件路径，正文之前逐个以 bracketed paste 写入路径、确认输入框出现 `[Image #n]`。`parts`（至少一项；给了就不能再给 `text`、`images`，否则 `bad_request`）是按顺序排列的 `{"text": …}` 与 `{"image": 路径}`，图片落在文字中间：每个图片路径都先检查；空的文字项跳过；没有图片时等同 `text` 为各段文字相连；有图片时，第一张图片之前的文字以 `!` 开头、或（不计开头空白）以 `/` 开头报 `bad_request`（`/` 开头的输入框被 Claude Code 当作斜杠命令运行，不是命令的则作为提示词发出但不带图片，2.1.318；图片在前时输入框是提示词，之后的文字不受限）。清空输入框后逐项以 bracketed paste 写入，每项等输入框显示出来才写下一项（文字：输入框有变化，只有空白的不必；图片：多一个 `[Image #n]`、路径离开输入框；连着写时图片会落到最后），`[Image #n]` 落在光标处、前后不加空格（空格由调用方给），最后回车；中途失败时清空输入框再报错；transcript 里这条用户消息的正文去掉 `[Image #n]` 后与各段文字相连相同（不计空白）、图片数也相同才算；进了队列的同样比对 enqueue 的正文与其中 `[Image #n]` 的个数 | `uuid`、`prompt_id`（插件看到的提交，没看到为 null）、`delivered_as`（`prompt`；进了 Claude Code 的队列为 `queued`，此时 `uuid`、`prompt_id` 为 null） |
| `steer {text, images?}` 或 `steer {parts}` | busy | 正文以 `!` 开头时报 `bad_request`（`parts` 同 `prompt`：第一张图片之前的文字以 `!` 或 `/` 开头）（忙时不发 `!` 命令）。先清空输入框再同样写入（`images` 同 `prompt`，正文之前逐个粘贴路径，队列里的这条以 `[Image #n]` 开头；`parts` 同 `prompt`，逐项粘贴，队列里的这条图片在文字中间）；忙时输入会排队，在当前工具调用结束后送达（transcript `queue-operation`/`enqueue`，随后作为 `queued_command` 附件交给模型）。等 enqueue（≤10s） | `delivered_as`：`queued`；若这时本轮恰好结束、被当作新提示词，则为 `prompt` |
| `follow_up {text, images?}` 或 `follow_up {parts}` | 除 exited | 进 ccrpc 队列；在两条命令之间、`idle` 已满 1 秒且上一轮已结束时按 `prompt`（刚 Stop 时 Claude Code 会先为它自己队列里的东西——如后台任务的通知——起一轮）（连同 `images` 或 `parts`，图片路径在入队时就检查）依次发出（不会在别的命令等待期间发出）；失败发 `follow_up_failed` 事件；`get_state` 的 `follow_ups` 与 `follow_up_failed` 的 `text` 对 `parts` 是各段文字相连、每张图片处写 `[Image]` | `queued`（队列长度） |
| `abort` | busy / waiting | 发 `Esc`；等状态离开 busy / waiting（≤10s） | `state` |
| `send_now` | busy 且 `queue` 不空 | 同 TUI 里排队时按 `ctrl+enter`（`ESC[13;5u`）：本轮中断（发 `aborted` 的 `turn_end`），队列里的内容作为新的一轮立即发出（没有自己的 UserPromptSubmit，由它的 Stop 结束）；等队列被取走（≤5s） | `{}` |
| `edit_queue` | busy 且 `queue` 里有人输入的内容（不以 `<cross-session-message`、`<agent-message` 开头） | 同 TUI 里排队时在空输入框按 `↑`：先清空输入框（输入框里有字时 `↑` 把取回的与它合在一起，且不取回 `!` 命令），按 `↑`，Claude Code 把队列里能编辑的各条（不含任务通知、别的 Session 或 Agent 发来的消息）取回输入框、以换行相连，transcript 里每条记一个带正文的 `popAll`；等这些记录（≤5s：`queue` 里不再有人输入的内容，或最后一条记录之后 0.3 秒，Claude Code 留在队列里的照旧留着），再清空输入框，取回的这几条不会在本轮之后发出（终端里按 `↑` 取回的也一样）。一条也没取回时（`↑` 翻出的是历史提示词）清空输入框后报错：这期间队列里的消息送出了报 `delivered`，否则报 `timeout`；取回了而输入框清不空时照常成功 | `items`：取回的各条（先入在前，正文同 `queue` 事件；带图片的含 `[Image #n]`，图片本身不返回） |
| `answer_permission {request_id, decision, message?}` | waiting.permission | 请求在插件里等着（主线）：`allow` 与 `deny` 经 hook 回决定——`deny` 不带 `message` 时中断本轮（同审批框的 No），带 `message` 时它作为反馈交给模型、本轮继续；`allow_always` 按审批框自己的“总是允许”选项（它加的规则如 `python3 *` 比 hook 能给的精确命令宽，见 probes.md），按键后放掉 hook，没有这一项则 `not_offered`。请求不在插件里等（后台 Subagent、hook 已超时、工具自己弹的对话框）：识别审批框按对应选项的数字键；无编号的菜单（computer use 的授权框）用 ↑ / ↓ 把 `❯` 移到该项、从屏幕上确认后回车。`message` 在拒绝、回到 idle 后作为 `prompt` 发出；工具自己的对话框拒绝后本轮继续，`message` 改作插话（`steer`）送达，本轮已结束或又停在别的对话框时排进 `follow_up` 队列 | `state`；按键拒绝且有 `message` 时另有 `message_uuid`（作为 `prompt` 发出）或 `message_delivered_as`（作为插话时同 `steer` 的 `delivered_as`，排进队列时为 `follow_up`） |
| `answer_question {request_id, answers}` | waiting.question | `answers` 与题目一一对应，`{"options":[label…]}`（多选以 `, ` 连接）、`{"other":"文本"}`，或多选题的 `{"options":[…],"other":"文本"}`（选项在前、文本在后，以 `, ` 连接）。hook 在插件里等着时作为 AskUserQuestion 的 `answers` 回给 hook（单选、多选都可 other）；没有 hook 在等时（提问出现时没有 ccrpc 连着、或 hook 已放行）按屏幕作答：按选项编号（多选按编号切换到要的勾选再到 Submit，多题用 `right` 换题、最后在 Review 按 Submit answers），other 按 `Type something.` 的编号、进入输入状态后写入再回车，多选题不支持 other（含与选项一起的，`unsupported`） | `state` |
| `answer_question {request_id, chat: true, answers?}` | waiting.question | 对话框的 `Chat about this`（先聊聊）：`answers` 同上，没答的题写 `null`，至少一题没答（都答了报 `bad_request`），不带 `answers` 即都没答。hook 在等时先放行（不给决定，对话框留着；hook 运行期间对话框不接数字键与方向键），等 0.8s 后按屏幕作答：依次答有答案的题、没答的用 `right` 跳过，到 Review 后用 `left` 回到第一道没答的题，按它的 `Chat about this` 编号；多选题同样不支持 other。Claude Code 把这次调用记为拒绝，结果里列出各题的答案或 `(No answer provided)`，本轮继续（模型接着问想澄清什么） | `state` |
| `slash {command}` | idle | 清空输入框，输入 `/<command>` 后回车（单行）；等命令离开输入框或被选择框顶替（≤2s），不等 transcript | `{}` |
| `set_model {model}` / `set_effort {level}` / `compact {instructions?}` / `clear` | idle | 即 `slash` 的 `model X` / `effort X` / `compact …` / `clear`（`level` 只能是一个字母数字词）。`/model X`、`/effort X` 也会成为新 Session 的默认（Claude Code 的行为）；对话中途第一次切换时 Claude Code 先问一句「历史要重新读一遍」，以内置布局的 `choice`（`Switch model?` / `Change effort level?`）出现，`answer_choice` 作答 | `{}` |
| `send_keys {keys}` | 除 exited | 依次写入，间隔见第 6 节。元素：命名按键（`enter` `esc` `tab` `shift+tab` `backspace` `space` `up` `down` `left` `right` `home` `end` `pageup` `pagedown` `delete` `ctrl+c` `ctrl+d` `ctrl+u` `ctrl+l` `ctrl+o` `ctrl+r` `ctrl+t`）、`{"text":"…"}`（原样）、`{"bytes":"<base64>"}` | `{}` |
| `get_state` | 任意 | — | `state`、`waiting`、`request`（挂起的请求：`request_id`、`tool_name`、`input`、`agent_id`；挂起的 choice 为它的 `choice` 事件，挂起的回退为 `rewind` 的响应）、`holder_id`、`session_id`、`cwd`、`follow_up_queue`、`follow_ups`、`size`（同 `ready`）、`mode`（输入框下模式行最后一次看到的，没看到过取插件报来的 `permission_mode`）、`model`（主线最后一条 assistant 的模型）、`context_tokens`（它的 input + cache_read + cache_creation）、`feedback_draft`（同 `feedback_draft` 事件，没有卡片时为 null） |
| `get_messages {since?, before?, limit?, subagents?}` | 任意 | 当前 transcript 的 Main Line 内容块，与从头跟随事件得到的一致（`branch` 作废的块不在其中）。由单独的线程维护：第一次（连接或切换 Session 时就开始）读完整个文件，之后只读新追加的部分，读的期间事件照常发出。`since`：只要这个游标之后写入的块（上次的 `cursor`）；`before`：只要这个游标之前的（上一页的 `first`）；`limit`：范围内最新的这么多块；`subagents`：另给本页 Agent / Workflow 调用下的 Subagent Run 内容块。游标是不透明的数（文件偏移），Main Line 变了也有效 | `blocks`、`cursor`、`first`（本页第一个块的游标，没有块为 null；同一条 transcript 条目的块总在同一页，页可能比 `limit` 略多）、`more`（范围内还有更早的块）；`subagents` 时另有 `subagent_blocks` |
| `get_screen` | 任意 | — | `lines`（每行去尾部空白）、`cursor`（`[行, 列]`，0 起） |
| `cancel_follow_up {index?}` | 任意 | 撤销 follow_up 队列里第 `index` 条（0 起），不带则全部撤销 | `removed`、`follow_ups`（剩下的） |
| `set_mode {mode}` | idle / busy | `manual` / `accept_edits` / `plan` / `auto`：按 shift+tab，每次等输入框下的模式行变化，直到显示目标模式；转一圈没有则 `not_offered` | `mode` |
| `get_tasks` | 任意 | 后台任务：后台 Subagent、后台 shell、Monitor、Workflow Run。以插件为准：PostToolUse 的工具结果开始它（Bash 的 `backgroundTaskId`、Agent 的 `agentId`、Monitor 与 Workflow 的 `taskId`；前台命令超时后转到后台的也算后台 shell；Subagent Run 里启动的不算，它的结果通知送给那个 Subagent），结果通知（UserPromptSubmit 的 `<task-notification>`）的 `status` 结束它，TaskStop 成功也结束它、记为 `stopped`（Claude Code 不为被停掉的后台 shell 发结果通知），SubagentStop 先记为 `completed`，Stop 的 `background_tasks` 补上没见到开始的（teammate（带 `name` 的 Agent）只在这里出现、类型 `teammate`，记作 `agent`；只经 Stop 知道的任务，之后的 Stop 不再列出就记为 `completed`；Claude Code 在那里把 Monitor 工具的任务记作 `shell`，见到过开始的仍按 Monitor；`monitor` 类型的是 Claude Code 自己的，如 Artifact 的实时更新，`/tasks` 里不显示，不列入）；Monitor 每个事件的通知没有 `status`，不结束它；连接前就有的从 transcript 读 | `tasks`：`id`、`kind`（`agent` / `shell` / `monitor` / `workflow`）、`tool_use_id`、`description`、`command`（后台 shell 与 Monitor）、`status`（`running`、Claude Code 给的结束状态，或 ccrpc 停掉后的 `stopped`） |
| `exit {close_pane?, stop_tasks?}` | idle | 输入 `/exit`，等 claude 进程消失（≤15s），发 `session_exit`；还有后台任务时 Claude Code 先问（`Background work is running`，选项含 `Exit and stop tasks`），`stop_tasks` 时在光标停在 `Exit and stop tasks` 上时回车，光标不在那里报 `unrecognized_screen`，没有 `stop_tasks` 时按 Esc 返回 `not_idle`；10 秒内 claude 既没退出、屏幕也不是这个问题时不按键：屏幕不是输入框报 `unrecognized_screen`，是输入框则再等它退出；`close_pane` 时再在 Pane 的 shell 里输入 `exit`，Pane 随之结束（Session 已 exited 时只做这一步） | `{}` |
| `stop_task {task_id}` | idle | 输入 `/tasks` 打开后台面板，按命令（后台 shell）或描述（Subagent、Monitor、Workflow）找到以它开头的唯一一个运行中的行（没有、多于一个、或有同样的已结束行时都不按键，报 `unrecognized_screen`；只有一项时面板直接是详情，Monitor 的详情按命令比对），一行一行移过去、确认光标在它上面后按 `x`；以面板为准确认它不再运行（≤5s），关面板；再等插件报来结果通知（≤2s，主线忙时要等当前工具结束），之前记为 `stopped` | `status`（通常 `killed`） |
| `dismiss_feedback_draft` | idle | 关掉输入框上方的反馈草稿卡片（卡片只在 idle 时显示）：另取一次 Snapshot，认出卡片（见 `feedback_draft` 事件），且卡片下方的输入框只有一行、`❯` 后没有字、光标就在 `❯` 之后，才按 `0`（卡片不在报 `no_feedback_draft`，输入框里有字报 `input_not_empty`，卡片下方看不到输入框——矮窗格里卡片把它挤出屏幕——时先把 PTY 调成 120×40 再看一次，仍看不到报 `unrecognized_screen`，都不按键）；等卡片消失或换成另一条草稿（≤3s），输入框仍为空才算成功，否则报 `input_not_empty` 或 `timeout`。卡片消失时发 `feedback_draft`（null）。草稿留在 `/feedback` 里，30 天后过期。不提供发送：卡片的 `2` 会把草稿发给 Anthropic，只在终端里按 | `{}` |
| `answer_choice {request_id, option}` | waiting（有挂起的 choice）；目录信任框时为 starting | 等对话框出现满 500ms，另取一次 Snapshot 确认屏幕上仍是该布局，`option` 按选项文字（完全一致，否则唯一前缀）找到编号按下；目录信任框按方向键把光标移到该项、回车，等它关闭 | `state` |
| `rewind {uuid}` | idle | 打开终端的 Rewind 选择器（清空输入框后按两次 `Esc`），停在 `uuid` 那条提示词的确认页：`uuid` 须是 Main Line 上人输入的提示词或斜杠命令、`!` 命令（按 transcript 库组装的 Main Line，即 CoDeck 显示的；斜杠命令写作 `/名字 参数`、`!` 命令写作 `! 命令`，与选择器一致；插话不算；上次恢复了对话、transcript 里还没有人发出新提示词时，那条及之后的不算），否则 `not_found`；读 transcript 后再确认一次仍是 idle。选择器从旧到新列出提示词与斜杠命令、`!` 命令，每项只显示第一行（多行或在屏幕边截断的以 `…` 结尾），光标从最下面的 `(current)` 起。按 `↑` 逐项上移（每次须恰好上移一行，否则 `unrecognized_screen`），项的文字（去掉 `[Image #N]` 与空白）与这条提示词的第一行相同、或以 `…` 结尾且是它的开头，就是候选：回车看它的确认页。确认页引用的提示词（不到 4 行即是全文，满 4 行可能截断、只比开头）对得上的提示词里没有这一条，就按 `Esc` 回到选择器继续上移；只有这一条即是它；有几条（同样的文字）时按遇到的先后对应（选择器里新的在下；其中有插话或命令时报 `ambiguous`：插话是否列出未实测，命令无从知道是否是忙时输入的），每遇到一页都核对它写明的发送时间与对应的那条对得上，对不上报 `ambiguous`（选择器列的与 transcript 不同，如在终端里回退过）。对应到这一条后，确认页写明的发送时间（`5s ago`）还须与 transcript 里它的时间对得上（前后各放宽 3 秒），否则 `ambiguous`。到顶仍没找到报 `not_in_picker`（如终端不列出的本地命令 `/context`）。任何一步失败都按 `Esc` 退出（只在屏幕上仍是选择器或确认页时按）。成功后终端停在确认页（waiting），这次回退成为挂起的请求（`get_state` 的 `request`），期间选择器与确认页不作为 `dialog` 上报、也不交给分析器（自己打开的选择器同样不交给分析器）；Session 不在等待或确认页不在屏幕上满 500ms（有人在终端里作答或按了 `Esc`），挂起的回退即结束，之后屏幕照常上报 | `type: "rewind"`、`request_id`（`rewind:N`）、`uuid`、`text`（这条提示词的全文）、`code`（确认页上代码会怎样的原文：`The code will be restored -1 in w.txt.` / `The code will be unchanged.`）、`warning`（有代码要恢复时的 `⚠` 警告原文，否则 null）、`options`（确认页上有的恢复选项 `[{option, label}]`：`both` = `Restore code and conversation`、`conversation` = `Restore conversation`、`code` = `Restore code`；没有代码改动时只有 `conversation`。确认页的 `Summarize from here` / `Summarize up to here` / `Never mind` 不提供） |
| `answer_rewind {request_id, option \| cancel: true}` | waiting（有挂起的回退） | 另取一次 Snapshot 确认屏幕上仍是这次的确认页（引用、代码一句、警告与选项都相同，否则 `unknown_request`；写明的发送时间会随重画变化，不比）。`option`：按下该项的编号键，等回到 idle、确认页与选择器都离开屏幕（10 秒）；恢复了对话时 Claude Code 把这条提示词放回输入框，等输入框出现这条提示词（全文，或长的开头至少 20 个字，≤3 秒，否则 `unrecognized_screen`；`!` 命令回到 shell 模式的输入框）；只恢复代码时输入框须仍为空。`cancel`：按 `Esc` 退回选择器、再按 `Esc` 关掉，等回到 idle | `state`；`option` 时另有 `text`（恢复了对话时为这条提示词或命令的全文，`!` 命令为 `! 命令`，可直接用 `prompt` 再发；只恢复代码时为 null） |
| `get_tool_result {tool_use_id}` | 任意 | 工具结果全文：落盘过的读 `persisted` 文件，否则取 transcript 里的结果；超过 100KB 截断 | `content`、`persisted`（没落盘为 null） |
| `get_image {uuid, index?}` | 任意 | 带图提示词的第 `index` 张图片（0 起，缺省 0；对应该 user block 的 `images`），从 transcript 里读 | `media_type`、`data`（base64） |

错误码：`bad_request`（JSON 或字段不对，含答案与题目对不上）、`unknown_command`、`not_idle`、`not_busy`、`not_waiting`、`unknown_request`（`request_id` 不是当前挂起的请求；`get_tool_result` 找不到该工具结果；`get_image` 找不到该图片）、`not_offered`（没有“总是允许”）、`unsupported`（提问不在插件里等）、`unrecognized_screen`、`input_not_empty`、`not_found`（`rewind` 的 `uuid` 不是 Main Line 上的提示词）、`not_in_picker`（终端的回退列表里找不到这条提示词）、`ambiguous`（回退列表里有分不清的同样提示词）、`delivered`（`edit_queue` 要取回的消息已经送出）、`timeout`、`exited`、`io`。状态不满足时立即失败，不排队；排队只有 `follow_up`。

### 5.2 事件

| 事件 | 字段 |
|---|---|
| `ready` | 见上 |
| `state` | `state`；`waiting` 时另有 `waiting: {kind, waiting_for}` |
| `block` | `kind`（`user` / `text` / `thinking` / `tool_use` / `tool_result`；Claude Code 存成 thinking 块的 narration——签名里的类型为 `narration`、TUI 按正文显示——是 `text`，也算作 `turn_end` 的 `last_text`）、`uuid`、`index`（消息内序号）；`text`；`tool_use` 的 `id`、`name`、`input`；`tool_result` 的 `tool_use_id`、`content`（文本，超过 100KB 截断）、`is_error`，输出太大被存进 `tool-results/` 时 `content` 是 transcript 里的预览（模型看到的也只有它），另有 `persisted: {path, size}`；带图的提示词另有 `images`（各张图片的 media_type，内容用 `get_image` 取；Claude Code 随后记下图片来源路径的 `[Image: source: …]` 条目不作为 block）；`user` 的 `text` 去掉了 Claude Code 给长粘贴加的 `<pasted_content id="…">` 标签，即写入时的原文；忙时插话送达的用户内容 `queued: true`；来自 Subagent Run 时另有 `agent_id`、`parent_tool_use_id`，属于 Workflow Run 时另有 `run_id`（`parent_tool_use_id` 为 Workflow 调用的 id） |
| `turn_end` | `prompt_id`：插件给的提示词 id（连接时已在进行的一轮为 null）；`trigger`：`prompt` / `task_notification`；`last_text`：Stop 的 `last_assistant_message`（StopFailure、中止为 null）；`error`：API 报错 `{kind, message}`（StopFailure 的 `error`，如 `rate_limit`、`server_error`、`unknown`），没有为 null；`aborted`：本轮被中止；`restored_text`：中止时 Claude Code 把这一轮的提示词放回了输入框（回复开始前按 Esc：提示词离开对话、回到输入框，transcript 里却留着它，下一条提示词也接在它后面），为这条提示词的全文（插件报来的），否则为 null——中止的一轮结束时看输入框，显示的是它（全文，或长的开头至少 20 个字）才算；`background_tasks`：Stop 时仍在运行的后台任务（Claude Code 原样：`id`、`type`、`status`、`description` 等），没有为 `[]`；`duration_ms`：`turn_duration` 的用时，没有则为 ccrpc 量的；`usage`：`input_tokens`、`output_tokens`、`cache_read_input_tokens`、`cache_creation_input_tokens`（本轮期间读到的主线与 Subagent Run 的 assistant 消息，每个 message id 计一次；连接时已在进行的一轮只算连接之后的部分）；`api_cost`：按 transcript 库的价格表估算的美元，有模型没有价格时为 null |
| `permission_request` | `request_id`（= `tool_use_id`）、`tool_name`、`input`、`agent_id`（后台 Subagent 的请求）、`options`（屏幕上审批框的选项 `[{label, meaning}]`，`label` 为全文，`meaning` 为 `allow` / `allow_always` / `deny` / `other`；认不出为 null）、`layout_source`（`builtin` / `learned`，`options` 为 null 时为 null）。主线的请求收到插件的 PermissionRequest 后最多等 1 秒认出审批框再发，认不出照发（allow / deny 仍经插件） |
| `question` | `request_id`、`questions`（AskUserQuestion 输入原样）、`agent_id` |
| `dialog` | `waiting_for`（进程文件原值；启动界面为 `startup`）、`lines`（屏幕）。启动界面每出现一个不同画面发一次 |
| `unrecognized` | `context`、`lines`；屏幕识别失败时发，不按任何键（PTY 不是 120×40 时随即调成 120×40，见第 7 节） |
| `follow_up_failed` | `text`、`error` |
| `queue` | `items`：Claude Code 自己的输入队列（插话、人在 TUI 里忙时输入的，先入在前；不含任务通知与 Teammate 消息），按 transcript 的 `queue-operation` 维护（`enqueue` 入队，`remove` 按正文移除，`dequeue` 取走最早的，`popAll` 按正文移除——`↑` 每取回一条记一个，不带正文的清空）；变化时发。`ready` 与 `get_state` 也带 `queue` |
| `branch` | Main Line 换到了另一个 Branch（rewind、编辑重发）。`at_uuid` 是此前发出的 block 里最后一个仍在 Main Line 上的 `uuid`，在它之后发出的 block 都已作废；为 null 时此前发出的全部作废。在新 Branch 的第一个 block 之前发。只由人发的提示词触发：它沿父条目往上（跳过附件等不产生 block 的条目）碰到的不是最后发出的那个条目时，按 transcript 库的规则以它为 Main Line 重算；后台结果通知、teammate 消息、compact 边界、父条目还没写出的条目不算。原样重发同一条提示词且没有回复（中止说明不算回复）的不发 |
| `workflow` | Workflow Run 的整份状态：`run_id`、`tool_use_id`、`name`、`status`、`phases`（`index`、`title`、`detail`）、`agents`（`agent_id`、`label`、`phase_index`、`state`）。`workflows/<runId>.json` 或 `journal.jsonl` 内容有变化时发，内容不变不发 |
| `choice` | `request_id`（`choice:<n>`）、`title`、`options`（`label` 屏幕上的全文、`meaning` `allow` / `allow_always` / `deny` / `other`、`description`）、`tool_use_id`（引起它的工具，如 ExitPlanMode；没有为 null）、`layout`、`layout_source`（`builtin` 内置 / `learned` 分析器学来的） |
| `layout_learned` | 分析器学到新布局并已存进布局库：`layout`、`kind`、`title`、`path` |
| `analysis_failed` | 分析器没给出可用的布局：`error`；结果与屏幕对不上时另有 `title` |
| `session_info` | `mode`、`model`、`effort`、`context_tokens`（同 `get_state`）：其中任一项变了时发。`model`：`/model` 的输出（`Set model to …`，此处或 TUI 里执行）一出现即为其显示名（如 `Sonnet 4.5`），之后的 assistant 消息给出模型 id；`effort`：输入框右上方的 `◉ xhigh · /effort`、`/effort` 的输出（`Set effort level to …`）或 assistant 消息记下的 `effort`，换了模型先清空，模型没有思考等级时为 null |
| `progress` | busy 时屏幕上的进度行：`text`（去掉符号的整行）、`verb`、`elapsed_s`、`tokens`（没有为 null）；文字变了时发，至多每秒一次（动词变了立即发） |
| `feedback_draft` | 模型用 SendFeedback 起草了产品反馈，Claude Code 在输入框上方显示卡片（从第 0 列画起的框，`✻ Product feedback drafted: <标题>`，末行 `1 to review · 2 to send · 0 to dismiss` 紧接底边，底边下方到输入框上沿或屏幕底只有空行或思考等级标记；对话里引用的同样文字不算）：`draft` 为 `{title, tool_use_id, input}`（主线最后一次 SendFeedback 调用，标题与卡片相同——卡片截断时以它开头——才带 `tool_use_id` 与 `input`，否则为 null；之后对上了会再发一次）。只在 idle 时判定：Claude Code 在本轮进行中隐藏卡片、对话框会盖住它，这期间草稿照旧算在；idle 时输入框显示而卡片不在满 1 秒、或经 `dismiss_feedback_draft` 关掉时 `draft` 为 null |
| `session_changed` | `session_id`、`previous`（`ready` 时还不知道 sessionId 的为 null） |
| `fork` | `new --fork` 做完时发一次（见第 2 节）：成功为 `session_id`（新 Session）、`origin`、`text`（回退时放回输入框的提示词全文，`!` 命令为 `! 命令`；没有 `--fork-at` 为 null）、`title`（改成的标题，没有 `--fork-title` 或改名失败为 null）、`title_error`（改名失败的原因，否则为 null）；失败为 `error: {code, message}`（`code` 同命令的错误码，`exited` 时 `message` 是 claude 退出前屏幕上的报错行） |
| `session_exit` | — |

- 不发 slash 命令的本地记录（`<command-name>` 等）与 `isMeta` 条目。
- `question` 在进程文件为 `input needed` 且插件报来了 AskUserQuestion 时发。
- `permission_request` 来自插件的 PermissionRequest（它没有 `tool_use_id`，按紧挨在前的 PreToolUse 的工具名、输入与 agent 配对）。
- 状态变化、或已发出的请求结束（插件报来它的 PostToolUse / PostToolUseFailure / PermissionDenied，连续两次审批可能合并成一段 waiting），都会重新认定并再发事件；状态离开 `waiting` 不算结束（进程文件可能抖动），回到 `idle` 时主线的请求都结束（后台 Subagent 的请求留到它的结果）。经插件作答后 1.5 秒内不重新认定（进程文件与屏幕还停在正在关闭的对话框上）。
- 连接前就已在等的审批（插件没报过）：在 transcript 未完成的工具调用里按同样的规则认定屏幕上的审批框，按键作答。
- 工具自己弹的对话框：Claude Code 自己的工具审批框都是编号菜单；无编号的审批框（computer use 的 `request_access` 授权框 `Computer Use wants to control these apps`）由运行中的 MCP 工具画出，插件只报它的 PreToolUse、没有 PermissionRequest（bypass 与 manual 模式实测都如此）。这类框只与它自己认定的请求对应（编号的审批框不会对上这种请求，反之亦然）：内置布局写明画它的工具（授权框为 `mcp__computer-use__request_access`），在 transcript 未完成的调用里恰好一个该工具时认定，即使插件报过它的 PreToolUse；分析器学来的无编号审批框不知道是哪个工具，只在未完成的工具调用恰好一个且是 MCP 工具（`mcp__` 开头）时认定；否则发 `dialog`。按键作答，回车只按一次（关得慢的框与丢了回车的框看起来一样），作答后这次调用不再参与认定。拒绝只结束这次工具调用（工具返回空授权），本轮继续。
- 按键作答的请求（后台 Subagent）按**屏幕上的审批框**认定：在这类请求里找命令（Bash）、文件名（Edit / Write / Read 等）、URL 或 pattern 出现在审批框正文里的那个；都没有可比对的内容时，只有唯一候选才认；1 秒内认不出就交给分析器或改发 `dialog`。作答前重新确认屏幕上的审批框仍对应该 `request_id`，按键前另取一次 Snapshot 再核对一次。

### 5.3 插件代答

- 插件的 PermissionRequest 等 ccrpc 一行回复。主线的审批与 AskUserQuestion：审批框与 hook 同时显示，ccrpc 扣住 hook 等调用方作答，回复决定后审批框收起（TUI 多一行 `Allowed / Denied by PermissionRequest hook`）。后台 Subagent 的请求（hook 阻塞期间审批框不显示）、ExitPlanMode（hook 只能拒绝）：ccrpc 立即回复“无决定”，界面照常出现，按屏幕作答。
- 放行：人先在 TUI 里作答（插件报来该工具的结果）、Session 回到 idle、ccrpc 断开时，ccrpc 放掉还扣着的 hook（无输出），审批框交还给人。PermissionRequest 的 hook 超时为 86400s；超时后审批框仍在，`answer_permission` 改为按键。

## 6. 写入

- 正文一次 bracketed paste（`ESC[200~ … ESC[201~`）写入，换行不会提交；等输入框里出现内容（≤2s，长正文显示为 `[Pasted text …]`）后再单独发 `\r`。4.5k 字符（含中文、换行）一次写入逐字正确。
- 按键之间至少隔 50ms，屏幕还没反应时最多等到 80ms。斜杠命令输入后等输入框出现该命令（≤1s）再回车，回车后等它离开输入框（≤2s）。
- 写入前去掉控制字符（保留 `\n`、`\t`，`\r` 换成 `\n`），防止注入转义序列。
- 写入前等屏幕上出现输入框（≤2s；刚启动时进程文件先说 idle、TUI 还没画好）。输入框上下两条横线；`/rename` 过的 Session 上边那条右端带名字（`──── 名字 ─`），也认作输入框。
- 清空输入框：按输入框行数重复“`ctrl+u` + 退格”，等屏幕上输入框变空（≤500ms，最多 4 次）；在 shell 模式（`!` 代替 `❯`，如回退到 `!` 命令后）清空后再按一次退格回到普通模式；不用 `ctrl+c`（连按两次会退出 Claude Code）。
- 读输入框内容时不算暗色的格（空输入框里的占位提示 `Try "…"`、补全的灰字）与紧挨在它前面的反显光标格。

## 7. 屏幕识别

- 布局库（[ADR-0002](./adr/0002-ai-learned-layouts-take-effect-at-once.md)）：内置布局在仓库 `layouts/*.json`，编译时嵌入（`CCRPC_BUILTIN_LAYOUTS=0` 关闭，测试用）；学到的布局在 `CCRPC_LAYOUT_DIR`，缺省 `$XDG_DATA_HOME/ccrpc/layouts`（`~/.local/share/ccrpc/layouts`），每个布局一个文件，含当时的屏幕与 Claude Code 版本。布局 = 问题行（原文）+ 选项（编号、文字的稳定开头、含义）+ 选法（`select`：缺省 `number` 按编号键；`cursor` 为无编号菜单，`❯` 用方向键移到该项再回车，编号即位置）。用前做结构检查：屏幕上有该问题行（窄屏折成至多 4 行时，去掉空白后连起来与之相同也算），其后是菜单：编号菜单为 strict 编号菜单（编号不超过 9），编号与布局一致、每项文字以布局的文字开头；无编号菜单为最下面一组依次以布局各项文字开头的行，两项之间的行算上一项的续行，菜单里不能有空行，`❯` 恰好在一项上；含义另有不依赖模型的字面检查：`No…` / `Deny…` 只能是拒绝，`Yes…` / `Allow…` 不能是拒绝（含 `auto mode` / `bypass` 的在审批类里只能是 `other`，在选择里不能是 allow_always），审批类恰好一个拒绝（编号菜单里须在最后），其余文字的选项在审批类里只能是 `other`；`Do you want…` / `Run a dynamic workflow?` 不能是 choice；审批类还要对得上一个挂起的工具调用。审批等待中只有挂起的工具是 ExitPlanMode 时才发 choice。学到的布局加载时重算 id、在它自己的屏幕上复核。
- 分析器：内置识别与布局库都认不出、且等待超过 250ms 时，把整屏文字、`waitingFor` 与挂起工具（插件报来的请求，没有则为 transcript 里未完成的工具调用）的输入发给模型（`CCRPC_ANALYZER_API_KEY`，`CCRPC_ANALYZER_BASE_URL` 缺省 `https://api.deepseek.com/anthropic`，`CCRPC_ANALYZER_MODEL` 缺省 `deepseek-flash`；没有 key 时关闭）。内置识别器认得的屏幕不送分析；审批等待中认不出满 1 秒才分析；同一对话框只分析一次，每小时最多 10 次，同时只有一个分析；分析期间不降级为 `dialog`。结果在它自己的屏幕上通过结构检查才存进布局库，立即使用，发 `layout_learned`；否则发 `analysis_failed`，照旧 `dialog`。每次分析都写一条分析记录（`CCRPC_ANALYSIS_DIR`，缺省 `$XDG_DATA_HOME/ccrpc/analyses`，再缺省 `~/.local/share/ccrpc/analyses`；一次一个 0600 文件，只留最近 200 条）：时间、sessionId、Claude Code 版本、模型与接口地址、用时、`waitingFor`、送出的屏幕与挂起工具输入、模型的回复原文、结果（learned 与存放路径，或 failed 与原因）。分析器也可给出无编号菜单（`select: cursor`）。工具审批归为 `permission_request`，其余菜单发 `choice`；没有挂起的工具调用时，分析器给出的审批类改作选择（斜杠命令的确认框看上去像工具审批，如 `/add-dir` 的）。内置布局含 computer use 的授权框（审批类、无编号，只认 `mcp__computer-use__request_access` 的调用：`Deny, and tell Claude` 拒绝、`Allow for this session` 允许，后者括号里是这次授权包含的应用数——窄屏时 Claude Code 只列得下部分应用，列不下的不在这次授权里），以及 `/add-dir` 的确认框 `Add directory to workspace`（选择：`Yes, for this session` 只在本 Session 加入、`Yes, and remember this directory` 加入并记住、`No` 不加入）。

- 屏幕 = 常驻的 `alacritty_terminal` VT：每收到 Holder 的 Snapshot 从头重建，之后的 Output 增量喂入；VT 尺寸跟 PTY 一致（按 Holder 的 Size 帧跟随，变了就重新要一次 Snapshot）。
- 读不懂时调成 120×40（[ADR-0001](./adr/0001-share-the-pane-and-resize-only-when-unreadable.md)）：PTY 跟随人的 Client，窄屏、矮屏下对话框会折行、被截掉，审批框的选项读不全，反馈卡片能把输入框挤出屏幕。以下情形 PTY 不是 120×40 时调成 120×40（Claude Code 按常见终端尺寸排版），等 800ms 让它重画再看：等待中的对话框内置识别与布局库都认不出（交给分析器、降级为 `dialog` 之前）；主线审批等满 1 秒仍读不出选项；写入前 2 秒内等不到输入框（之后再等 2 秒，没按过键）；`dismiss_feedback_draft` 看不到卡片下方的输入框；以及其余命令读不懂屏幕、报 `unrecognized_screen` 时（已按过键的不重试，调用方再发一次即在新尺寸下读）。调过就不调回去，人的 Client 下次抢尺寸时换回它的尺寸；已经是 120×40 仍读不懂就照旧报。
- 审批框：`Do you want …?` 之后的编号菜单；Workflow 的审批框问 `Run a dynamic workflow?`，菜单在脚本说明之后（说明里可能有编号的阶段，取其后最后一个从 1 开始的编号块），选项为 `Yes, run it` / `View raw script` / `No`。按文字判断含义：`Yes`（Workflow 为 `Yes, run it`）→ allow；`Yes, …` 且含 `always allow` / `don't ask again` / `allow all`、不含 `auto mode` → allow_always；`No` 开头 → deny。必须同时有 allow 与 deny，否则不认。编号随场景变化，绝不按固定编号。
- AskUserQuestion：底部 `Enter to select … Esc to cancel` 提示行、上方 `☐` / `☒` 标签行（`←` 开头表示多题）、题目、编号选项（`[ ]` / `[✔]` 为多选）、`Type something`、`Chat about this`、多选题的 `Submit` 行；Review 标签为 `Review your answers` 与 `Submit answers`。
- 输入框：最下面两条整行 `─` 之间、以 `❯`（shell 模式为 `!`，读出的内容以 `!` 开头）开头的内容。agents 页面（`tests/fixtures/screens/agents_list.txt`）底部同形的框是新开后台 Session 用的，不算输入框：框里是 `describe a task for a new session`、下方提示行含 `ctrl+x to delete` / `ctrl+x to stop` / `ctrl+enter to start and open` / `space to reply` 时认作 agents 页面；转入后台后（P 有 `parkedJobId`）顶部的 `Your conversation moved to the background` 与左边顶格的 `Needs input` / `Working` / `Completed`（收起时带数目）分段标题也算，并且要求框下一行是模式行（`… mode on` / `← for agents`）才算输入框（输入斜杠命令时补全列表在框上方，模式行仍在框下，2.1.318）。认不出时要写入的命令照常报 `unrecognized`、不按键（也不会用 ctrl+u 清掉人在 agents 页面框里写了一半的任务）。
- Rewind（2.1.318，`tests/fixtures/screens/rewind_*.txt`）：选择器是紧接在对话框横线（`▔▔▔`）下的最下面的 `Rewind` 标题行（屏幕上有输入框时不算）、其下 `Restore the code and/or conversation to the point before…`，到 `Enter to continue · Esc to cancel` 为止；其间以空行分组，每组第一行是一项（`❯` 为光标），下一行是这一轮改了什么（`No code changes` / `w.txt +1 -1` / `⚠ No code restore`）；只画出一段，`↑ N more` / `↓ N more` 为上下还有几项。确认页是 `Rewind` 标题行下 `Confirm you want to restore to the point before you sent this message:`（Session 没有文件历史、恢复不了代码时为 `Confirm you want to restore the conversation to the point before you sent this message:`，菜单只有 `Restore conversation` 与 Summarize、`Never mind`），其后 `│ ` 开头的行引用这条提示词（末行 `(5s ago)` 不算），然后 `The code will be …` 一段、编号菜单（项多时滚动，被挤到边上的项以 `↓` / `↑` 代替光标列；折行接在上一项后）、`⚠ …` 警告；菜单须恰有一个 `❯`、编号连续。

## 8. 测试

- 单测：`tests/fixtures/` 的屏幕文本与 transcript（M0 录制、脱敏）回放：transcript 尾随、屏幕识别、协议解析。
- 端到端（`tests/e2e/`：`harness.rs` 是隔离环境与收发，`mock_model.rs` 与 `real.rs` 是用例；`#[ignore]`；`mock_` 开头的用例用测试内置的本地 mock 模型（`MOCK-TOOL` 调 Bash、`MOCK-ASK` 调 AskUserQuestion 等），其余用真实模型；设 `CCRPC_E2E_WIRE_DIR` 时把每个 ccrpc 的收发原样录成 `<用例>-<n>.jsonl`（`{t, dir: send|recv, msg}`，与 CoDeck 的 ccrpc 日志里的 wire.jsonl 同格式）供 CoDeck Connect 功能的 E2E 回放）：ccrpc 插件为本次构建生成、经 `claude --plugin-dir` 加载（不安装），独立 `CCRPC_STATE_DIR`（`/tmp` 下）；独立 `TOH_STATE_DIR`（`/tmp` 下的短路径，unix socket 路径有约 104 字节上限；clay 启用、`[lan] bind = "127.0.0.1"`、`mdns = false`，模型相关变量经 `[backends.clay] env` 传给 Pane），去掉 `TOH_CONFIG` 与宿主终端变量，Holder 与 socket 一律经 `toh ls --json` 取；独立 `CLAUDE_CONFIG_DIR`（预置跳过 onboarding 与目录信任、`permissions.defaultMode = "default"`）；模型缺省 DeepSeek `deepseek-flash`（`https://api.deepseek.com/anthropic`），key 读 `CCRPC_E2E_API_KEY`。清理只向记下的 Pane shell pid 发 SIGHUP（Pane 随之结束）。
- 转入后台的端到端用例（`mock_model_follows_*`）：后台 claude 跑在 Pane 里的 claude 拉起的 daemon 下，daemon 按配置目录隔离（`/tmp/cc-daemon-<uid>/<配置目录的 sha256 前 8 位>`），碰不到主人的；用例结束、Pane 关掉后，按 `--spawned-by` 里记下的 Pane 里的 claude 的 pid 找出这个 daemon 及其子进程，先 SIGTERM、10 秒后 SIGKILL，再删掉它的 socket 目录。

- 权限模式：输入框下的模式行，`manual mode on` / `accept edits on` / `plan mode on` / `auto mode on`，shift+tab 依次循环。
- 后台任务面板（`/tasks`）：标题 `Background`，下面的行带 `❯` 光标，提示行 `↑/↓ to select · Enter to view · x to stop · Esc to close`；只有一项时直接是详情（`… details`，提示行同样有 `x to stop`）。

## 9. 已知限制

- 屏幕布局按 2.1.316 / 2.1.317 识别；Claude Code 改了文字或布局时识别器会拒绝（`unrecognized`），需要更新 recognize.rs 与 fixture。
- 转入后台后，Pane 改为显示另一个后台 Session（在 agents 列表里打开别的）时进程文件不变，ccrpc 认不出，仍跟着转入的那个：这时发的提示词进了 Pane 显示的 Session，等 30 秒报 `timeout`。
- 转入后台后的 `exit`：`/exit` 只让 Pane 回到 agents 列表（后台 claude 照常运行），报 `unrecognized`；在列表里 `ctrl+c` 两次才结束 Pane 里的 claude，后台 claude 仍留在 daemon 里。
- 后台 claude 被停掉（列表里 `ctrl+x`）到再打开之间没有它的进程文件，状态停在最后的值；这期间 Pane 显示 agents 列表，要写入的命令报 `unrecognized`。
- 配置目录的 daemon 若是由 clay 外的 claude 拉起的（它的环境里没有 `TOH_HOLDER_ID`），后台 claude 里的插件 hook 按「只在 clay Pane 里生效」直接退出、不报到，转入后台的对话就一直跟不过去（`session_changed` 不来，写入的提示词等 30 秒报 `timeout`）；结束那个 daemon（或从 clay 里的 claude 重新拉起）后才行。
- NEW 里复制部分之后、B 写出第一条带 uuid 的条目之前，没有 uuid 的条目（如 `queue-operation`）也当作复制来的：这期间插话的入队记录不用来确认命令。
