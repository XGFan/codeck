# ccrpc

让 Host 本机的脚本像 Pi RPC 那样驱动交互式 Claude Code：Claude Code 跑在 ToH 的 clay Pane 里，ccrpc 只经 Holder 写 PTY、只读 transcript / 进程文件 / 屏幕，Claude Code 对此无感知。

Session、Live Session、Subagent Run、Main Line 沿用 [Claude Viewer 的定义](https://github.com/XGFan/claude-code-viewer/blob/main/CONTEXT.md)；Host、Holder、Pane、clay、Client 沿用 [ToH 的定义](https://github.com/XGFan/toh/blob/main/CONTEXT.md)。下面只列本项目新增的词。

## Language

**受控 Session（Controlled Session）**:
有 ccrpc 进程连着的 Session。它一定在本机某个 clay Pane 的 Holder 里运行；ccrpc 退出后它仍在运行，只是不再受控。
_Avoid_: managed session、worker（worker 是 paaa 的词）

**接手（Takeover）**:
人在 ToH Client 上操作 Session 所在的 Pane：把它切到前台，或在其中按键。正被人接手的 Session 不能成为受控 Session；受控 Session 被接手后，ccrpc 对它只读，直到调用方取回（接手的人已离开时）或重新连接。
_Avoid_: 抢占、冲突、人机协同

**调用方（Caller）**:
经 stdin / stdout 与 ccrpc 交换 JSONL 的程序。
_Avoid_: client（Client 是 ToH 的 Mac / iOS / web）、host（Host 是 ToH 的机器）

**状态（State）**:
受控 Session 当前所处的阶段：`starting` / `idle` / `busy` / `waiting` / `exited`。前四种取自进程文件的 `status`，`waiting` 另带子类（`permission` / `question` / `dialog`）与原始 `waitingFor`。
_Avoid_: 状态码、status（指本项目的状态时）

**进程文件（Process File）**:
Claude Code 为每个运行中的进程写的 `<配置目录>/sessions/<pid>.json`，含 `sessionId`、`status`、`waitingFor`。ccrpc 判断状态的主要依据。
_Avoid_: pid 文件、session 文件

**内容块（Block）**:
transcript 里一条消息内的一个内容单元：用户文字、assistant 文字、thinking、工具调用、工具结果。事件的最小粒度。
_Avoid_: chunk、delta、token

**请求（Request）**:
受控 Session 停下来等人作答的一次交互：审批（permission）或 AskUserQuestion（question）。以触发它的 `tool_use_id` 为 `request_id`。
_Avoid_: prompt（prompt 指调用方发的提示词）、dialog（dialog 指其余对话框）

**选择（Choice）**:
受控 Session 停下来等人在一个编号菜单里选一项、但既不是审批也不是 AskUserQuestion 的界面，如计划审批、退出确认。不以 `tool_use_id` 标识（可能由工具引起，如计划审批），按选项文字作答。
_Avoid_: dialog（dialog 指认不出含义、只能按键处理的界面）、request

**布局（Layout）**:
ccrpc 认得的一种界面：怎么认出它、它是请求还是选择、每个选项的含义与按键。来自实测，或由 AI 分析认不出的屏幕得出并存下，之后同样的界面直接按它识别。
_Avoid_: 模板、规则、pattern

**屏幕（Screen）**:
ccrpc 用自己的 VT 从 Holder 的 Snapshot 与 Output 维护的当前终端画面文本。只用来识别界面与选项位置，不作为内容来源。
_Avoid_: 截图、output
