# TODO

## 进行中：输入输出机制优化（ccrpc、toh、claude-viewer 一起改）

前提：ccrpc 不与人共用 Session（见 CONTEXT.md「接手」）。开发期 ccrpc 用 `[patch]` 指向本地的 toh 与 claude-viewer，稳定后再推 GitHub、去掉 `[patch]`。端到端把本地构建的 toh 放在 PATH 最前面（Holder 由它拉起），不动 `~/.local/bin/toh`。

ccrpc：

- 屏幕：用 Output 流维护常驻 VT（附着时用 Snapshot 打底），固定等待换成“等屏幕出现预期变化”；按键作答前仍取一次新快照核对
- 尺寸：附着后发 `Resize` 成为尺寸所有者，缺省 160×48，`--size COLSxROWS` 可改（spec 去掉“不发 Resize”）；发现尺寸被人改了只重新同步，不抢回
- 接手：attach 时尺寸所有者是另一个在线 Client 就拒绝（`--force` 跳过）；连着时被接手发 `taken_over`，之后写入类命令返回 `taken_over` 错误、清空 follow_up 队列，事件照常发；旧 Holder 不报尺寸所有者时放行并在 stderr 提示，只靠尺寸变化判断接手
- 落盘的大工具结果：`tool_result` 的 `content` 保持预览（与模型所见一致），加 `persisted: {path, size}`；新增 `get_tool_result {tool_use_id}` 返回全文（100KB 截断）
- Branch：新增 `branch {at_uuid}` 事件（此前发出、在 at_uuid 之后的 block 已不在 Main Line）；`get_messages` 只返回 Main Line；原样重发且无回复的 Branch 不发
- Workflow Run：读 `subagents/workflows/<runId>/agent-*.jsonl`（目前漏读，其中的 block 没有发出），block 带 `run_id`；`workflows/<runId>.json` 内容变化时发 `workflow` 整份快照（run_id、tool_use_id、status、phases、agents）
- Subagent 文件长度没变就不打开，结束后停止轮询
- 轮询改为文件事件驱动
- 端到端补 Subagent 与 Workflow 用例
- 改用 cv-core 的进程文件、行读取与 transcript 路径解析，去掉自己的实现

toh：

- Attach 可声明订阅尺寸所有者；Holder 订阅后立即回一帧当前所有者（client 标识，没有为空），之后每次换人再发。不订阅的 Client（Mac、iOS、web）不收，toh-proto 的解码器遇到未知帧会报错

claude-viewer（cv-core）：

- 进程文件补 `waitingFor`、`cwd`、`version`，Live 状态加 Waiting；界面上 waiting 用实心琥珀色点，提示「进行中 · 等待作答」（Session 列表与搜索结果）
- 增量的 Main Line 判定：追加条目后给出哪些在 Main Line 上、Branch 从哪里切换
- Workflow Run 与 Subagent 的结构（阶段、归属、嵌套）支持增量使用
- 开放按引用读落盘工具结果全文的接口

## 下一步：使用方（协议之上的子项目，各自单独设计）

- 主控 Claude Code 调度 Claude Code Worker（类 paaa）：派 Task、取 Result、Ask / Approval 转给主控、插话、中止；事件以 monitor 推给主控；人可随时在 ToH 上接手
- ToH Client 的结构化操作：Session 状态（忙 / 闲 / 等审批）、审批卡片、发提示词、长任务完成推送（对应 ToH TODO 的「长任务完成通知推送」）
- 远程调用：经 ToH daemon 暴露，复用它的令牌、配对与 tailnet

## 协议与形态

- 同一 Session 连多个 ccrpc
- 常驻进程 + unix socket 形态（一条连接管多个 Session，沿用同一套 JSONL 消息）

## 已知限制（MVP 遗留）

- 多选题的 Other（自由文本）作答：先实测布局与按键，目前返回 unsupported
- /clear 后约 1.3s 内（新 sessionId 被察觉之前）发出的 prompt，其 block 落在“历史”里不发出（prompt 本身照常成功）
- 第一次查找 transcript 时偶发的文件系统错误会让已有 transcript 之后按实时读，重放历史 block
- 同一个审批框期间进程文件若 waiting→busy→waiting 抖动，该请求会降级为 dialog，需用 send_keys 作答
- 连续两次审批合并成同一段 waiting 的情形只做了逻辑处理，没有端到端用例

## 维护

- 建远程仓库并推送（目前只有本地提交；cv-core、toh-proto 的 git 依赖已指向 GitHub）
- Claude Code 升级后重跑端到端；识别失败时用 `ccrpc debug screen` 拍新布局存进 fixture，更新 recognize.rs 与 probes.md 的版本
