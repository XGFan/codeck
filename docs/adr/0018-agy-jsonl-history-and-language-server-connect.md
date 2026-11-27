# agy：历史读它的 JSONL，Connect 接它进程内的 language server，不走 ACP

CoDeck 同时支持 agy（Google 的 Antigravity CLI）。和 Claude Code 一样，Connect 的对象是人在终端里开着的那个 TUI，不另起无头进程，也不走 ACP。Session 多一个属性 **Agent**（`claude` 或 `agy`），agy 的 Session ID 就是它的 conversation id。以下依据 2026-11-27 对 agy 1.5.18 的实测。

**历史只读 agy 落盘的文件**，数据根 `~/.gemini/antigravity-cli`：

- 列表：`cache/conversation_metadata.json`，每个会话的标题、workspace、更新时间、步数（本机有 JSONL 的 188 个会话全在里面）。
- 内容：`brain/<id>/.system_generated/logs/transcript_full.jsonl`，每个 step 一行（类型、来源、时间、模型、token、思考、工具调用、正文）。agy 给 hooks 的 `transcriptPath` 就是它。同一个 step 下标再次出现时以后一行为准。工具结果不带调用 id，按顺序对上前一条回复的 `tool_calls`：本机 1756 条带工具调用的回复里 1740 条严格对齐，其余 16 条是会话末尾还没写出结果；和 SQLite 里的 steps 相比，8153 个 step 只缺 19 个。
- Live：有 agy 进程持有 `presence/<id>.lock`，即那个进程的 TUI 当前打开着这个会话。

**Connect 时 CoDeck 是 agy 内置 language server 的另一个客户端。** 每个 agy 进程都在 localhost 的随机端口上跑 Connect-RPC 服务，TUI 用的也是它：

- 鉴权：agy 启动时环境里有 `ANTIGRAVITY_SERVER_CSRF_TOKEN`，就用它当 token；没有时随机生成，外面拿不到。用户在 shell 里设一次（`set -Ux ANTIGRAVITY_SERVER_CSRF_TOKEN (openssl rand -hex 16)`），CoDeck 自己起的 agy（新建、Resume）自带。CoDeck 从进程的启动环境读出这个值，端口取该进程监听的 HTTP 端口。没带 token 启动的 agy 只能只读跟随。
- 状态：`StreamAgentStateUpdates`，首条是全量快照，之后按 step 下标整条替换，带运行状态、等待中的审批与提问、排队的消息、后台任务、上下文用量。对话内容仍来自 JSONL 的实时跟随，与 ADR-0007 一致。
- 控制：发消息 `SendUserCascadeMessage`，默认插进当前这一轮的下一次模型调用，`WHEN_IDLE` 排队等空闲后开新一轮，排队的可用 `DeleteAgentMessage` 撤回；审批与提问 `HandleCascadeUserInteraction`，TUI 的对话框随之关闭并写「Answered from another device.」，拒绝可附理由，模型在同一轮里看得到并继续；中止 `CancelCascadeInvocation`，等于在 TUI 里按 Esc，`killBackgroundTasks` 连后台命令一起停。
- **按会话的调用只发给持有该会话 presence 锁的进程。** 别的进程一收到订阅或读 steps 就会载入这个会话、打开同一个 `.db`；再发消息会在后台另跑一条线，实测 SQLite 与 JSONL 里同一个 step 下标被两边写乱。锁不在任何进程手里时只读、不 Connect。公开的 `product.v1.ConversationService/GetConversation` 不会载入会话，可以向任意进程查运行状态。
- 模式与模型跟随 TUI 当前的选择，流里读得到实际生效的值。模式可以切换：Session 在 ToH clay 里时往 Holder 发 shift+tab，直到 TUI 底栏显示目标模式；不在 clay 里时不能切。模型只显示：本地服务没有设置它的方法，切换要驱动 TUI 的 `/model` 选择界面。
- 接口定义：agy 的二进制里嵌着全部 proto 的 FileDescriptorProto（1.5.18 有 379 个文件），按所用版本导出、生成类型，不认识的字段忽略。

## Considered Options

- **ACP（`agy acp`）**：haa 用的方式，接口有文档；但它是另起的进程，人在终端里开着的那个 TUI 接不上，手动开的 Session 也就无法 Connect。
- **`agy -p` 的 stream-json**：无头，问题同上，而且没有权限请求的回程。
- **ccrpc 式驱动 TUI**（往 PTY 写键、hooks 报事件、读屏识别审批与提问）：与 Claude Code 同构，不依赖内部接口；但要为 agy 的 TUI 另写一套界面识别，TUI 一改版就坏。hooks 也给不了审批的回程：让 PreToolUse 阻塞着等 CoDeck，TUI 就不再弹出自己的确认。
- **历史读每个会话的 SQLite**（`conversations/<id>.db`，steps 是 protobuf）：信息最全，但 `crates/transcript` 要引入 SQLite（ADR-0007 特意让 ccrpc 不再编译它）并按 protobuf 解码；而且这些是 WAL 库，agy 开着时只读连接也要写 `-shm`，没开时 `mode=ro` 打不开。JSONL 的覆盖率已经够，缺的调用 id 用顺序补上。
- **agy 自带的 remote control**：经 Google 的云端中转（`jetski-webchannel.googleapis.com`），不在本机。
- **经 hooks、插件的 MCP server 或 sidecar 拿 token**：实测前两者的环境里没有 language server 的地址与 token，CLI 不运行插件的 sidecar。

## Consequences

- 依赖未公开的内部接口（`exa.language_server_pb.LanguageServerService`），而 agy 发版很勤（11-23 是 1.5.14，11-26 已到 1.5.18）。1.5.17 → 1.5.18 在所用方法涉及的 1062 个类型上没有破坏性变化，只多了 2 个字段。Connect 时核对 agy 版本；接口变了只影响 agy 的 Connect，历史不受影响（它只依赖 hooks 文档给出的 JSONL）。
- shell 里设的是一个固定值，所有 agy 进程共用这个 token。agy 本来就把各自的 token 交给 agent 跑的 shell 命令，agent 也能以用户身份运行任意命令，所以不新增权限。
- 人在 TUI 里忙时输入的排队只在 TUI 本地，服务看不到，CoDeck 显示不了也撤回不了；CoDeck 是唯一操作方，不处理这种情况。
- `~/.gemini/antigravity-cli` 和 `~/.claude` 一样严格只读（ADR-0002），CoDeck 不打开 agy 的 SQLite。
- JSONL 里只有 token 数、没有单价，agy 的 Session 怎么算 Cost 另定；流里有 agy 自己给的花费汇总，金额尚未核对。
- CONTEXT.md 加 Agent；Session、Live Session、Connect 的定义在实现时扩到 agy。
- 审批里的「本会话内一直允许」「写进 settings.json」、回退、Fork、子 agent 还没实测，做到时再验证。
