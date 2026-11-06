# CoDeck

一个只读的 macOS 桌面应用，用来搜索和阅读 Claude Code 在本机留下的历史对话记录。

## Language

**Session**:
一次 Claude Code 对话，以 session ID 为唯一身份；无论它的记录文件在磁盘上出现几份，都只算一个 Session。
_Avoid_: 会话文件, conversation, chat, transcript（指整体时）

**Project**:
一个 git 仓库（worktree 归入主仓库），不在仓库里的目录则是该目录本身；以解析符号链接后的路径为身份，而非 `~/.claude/projects/` 下的目录名。一个 Session 属于它启动时所在的 Project，以及它改动过文件的每个 Project。
_Avoid_: 项目目录, workspace, folder

**Live Session**:
仍有 Claude Code 进程在运行、可能继续追加内容的 Session；其余 Session 视为已结束。侧栏顶部列出它们的那一块就叫「Live」。
_Avoid_: 活跃会话, active session, 进行中的会话, 运行中 / Running（指侧栏那一块时）

**Connect**:
经 ccrpc 连上一个在 ToH clay 里运行、装了 ccrpc 插件的 Live Session，从此可以在应用里向它发提示词、作答审批与提问。
_Avoid_: 驾驶, attach（ToH 的 attach 指把终端接到 Pane）, 接管

**Connected Session**:
已 Connect 的 Live Session，即 ccrpc 一侧的受控 Session；对话内容仍来自索引与实时追随。
_Avoid_: 被驾驶的 Session, 受控 Session（文案中）, 控制模式

**Disconnect**:
结束 Connect，Session 照常运行。
_Avoid_: 断开, detach

**Resume**:
让一个已结束的 Session 重新运行（`claude --resume`）并 Connect 它；Session ID 不变，仍是同一个 Session。
_Avoid_: 恢复, 继续, 重启

**Subagent Run**:
Session 内由一次 Task/Agent 工具调用派生出的子对话；它属于该 Session，不是独立的 Session。
_Avoid_: sidechain, 子会话, agent session

**Workflow Run**:
Session 内由一次 Workflow 工具调用编排的一组 Subagent Run，按阶段组织；它和其中的 Subagent Run 都属于该 Session。
_Avoid_: 工作流会话, pipeline

**Branch**:
同一个 Session 内，用户回退（rewind）或编辑重发后产生的另一条后续路径；它不产生新的 Session ID。
_Avoid_: Fork（Fork 是跨 Session 的）, 版本, 分叉会话

**Main Line**:
Session 中从第一条消息走到最新叶子消息的那条 Branch，即用户在终端里最终看到的对话。
_Avoid_: 主分支, 当前分支, trunk

**Teammate Message**:
Session 中由另一个 Claude Session（agent team 的成员）发来的消息；它以用户消息的形式记录，但不是用户输入。
_Avoid_: 用户消息, peer message, 队友输入

**Fork**:
从另一个 Session 复制了部分历史后继续进行的新 Session；被复制的那个称为它的 **Origin Session**。Claude Code 的 `/branch` 命令与 `--fork-session` 产生的都是 Fork。
_Avoid_: 分支, Branch（虽然 `/branch` 这样叫）, 副本, Resume（Resume 不产生新 Session，不算 Fork）

**Continued Session**:
Session 在 agents 页面转入后台（Move to Background）时 Claude Code 新起的 Session：它复制了原 Session 的全部对话并接着进行，原 Session 由此**被接替**（superseded）。两者是同一个对话，只显示 Continued Session，打开原 Session 时转到它；它不是 Fork，也不标 Origin Session。
_Avoid_: Fork（不是分出去的另一条路）, 副本, Resume（Resume 不产生新 Session）

**Cost**:
按 API 公开单价估算的花费（美元），由每条 assistant 消息的模型和 token 算出；同一次 API 调用出现在多个 Session 中时只计一次，归入最早的那个 Session（被接替的除外）。它是估算值，不是实际账单（订阅用户并不按此付费）。界面与文档里一律写 Cost。
_Avoid_: API 金额（「API」有歧义）, 金额, 费用, 花费, 账单
