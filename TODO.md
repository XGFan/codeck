# TODO

## 发布前（当前改动收尾）

- 把 toh（`db2656c`、`6d3a1f3`）与 claude-viewer（`1f1ceb3`、`64c04f6`）推到 GitHub；ccrpc 去掉 `Cargo.toml` 的 `[patch]`、`cargo update` 锁定新提交
- ccrpc 建远程仓库并推送
- 本机 `~/.local/bin/toh` 换成带尺寸所有者通知的版本；之前起的旧 Holder 只能靠 PTY 尺寸变化判断接手

## 兼容升级与协议补全（已落地，后续）

- ccrpc 插件：PostToolUseFailure / PermissionDenied 结束请求、hook 已超时后回落按键，都没有端到端用例；Claude Code 升级后先重跑 probes.md 的 hooks 一节（hook 输入字段、阻塞时界面是否照常显示）
- 学到的布局只在本机：常见界面学到后用 `ccrpc layouts export .` 导出成内置布局，提交前检查其中的整屏文字
- 分析器只处理“等待作答的对话框”；输入框、模式行、后台任务面板仍是代码里的识别，Claude Code 改了这些界面要改 recognize.rs
- `stop_task` 停 Subagent 只按面板上的描述匹配，没有端到端用例（目前只测了后台 shell）
- 被 ccrpc 停掉的后台任务在主线忙时先记为 `stopped`，Claude Code 的结束通知要等当前工具结束才送达
- 全屏 TUI 下拍到的输入框、审批框、提问框、后台面板与默认 TUI 一致；计划审批只在默认 TUI 下实测

## GUI MVP 要的改动（只在本机用，不发布）

GUI 是另起的 Tauri 应用 cc-gui（`~/Developer/Joy/cc-gui`，设计见它的 docs/spec.md），内容只来自 ccrpc（`get_messages` + `block` + `branch`），每个运行中的 clay Claude Code 各连一个 `ccrpc attach --co-display`；TUI 同时在 Mac Client 里显示，认不出的界面在 TUI 里手动处理。

已做：`ls --json`（带标题）、`get_state` 的 `mode` / `model` / `context_tokens` 与 `session_info` 推送、`--co-display`、目录信任框作为 choice、`exit {close_pane, stop_tasks}`、`--log`、`progress` 事件、`get_messages` 的分页 / 增量缓存 / 后台线程 / `subagents`、`permission_request` 的 `options`、`choice` / `permission_request` 的 `layout_source`、`--log` 补足（版本、识别决策、整屏）、mock 端到端录 wire（`CCRPC_E2E_WIRE_DIR`）。

- rewind：`rewind {uuid, restore}` 经选择器作答（先实测提示词多时选择器的滚动与截断、恢复代码的确认菜单变体）
- 认不出的界面：v0 先经分析器归类，归不了的 GUI 只显示屏幕原文、在 TUI 里处理（不在 GUI 里按键）
- 斜杠命令、模型的候选列表
- 去掉共显前：GUI 覆盖所有对话框

## 形态

- 常驻进程 + unix socket（一条连接管多个 Session，沿用同一套 JSONL 消息）
- 同一 Session 连多个 ccrpc
- 调用方 SDK（Rust、TypeScript / Python）与示例
- 分发：版本号、CHANGELOG、release 二进制

## 使用方（协议之上的子项目，各自单独设计）

- 主控 Claude Code 调度 Claude Code Worker（类 paaa）：派 Task、取 Result、Ask / Approval 转给主控、插话、中止；事件以 monitor 推给主控；人可随时在 ToH 上接手
- ToH Client 的结构化操作：Session 状态（忙 / 闲 / 等审批）、审批卡片、发提示词、长任务完成推送（对应 ToH TODO 的「长任务完成通知推送」）
- 远程调用：经 ToH daemon 暴露，复用它的令牌、配对与 tailnet

## 待确认

- 忙时在 TUI 里按 Enter 的输入何时送达：probes.md 记为当前工具结束即插进本轮（`absorbed_mid_turn`，即 steer），但日常使用的体感是本轮结束后才发（像 follow_up）；在当前版本重测，分清是版本变化、还是多数时候本轮已没有工具调用可夹带

- ToH Client 会不会把 `ccrpc new` 刚建的 Pane 自动切到前台并抢尺寸（会的话 ccrpc 一连上就被判为接手）
- 后台挂着的 Mac / web Client 上次操作过的 Pane 仍算“有人在用”，`attach` 要 `--force`；是否改为按最近一次输入时间判断（需要 Holder 报告）
- 按键间隔现在常是 50ms（忙时 spinner 也算屏幕有反应），端到端都过；换慢机器或更长菜单时留意

## 已知限制

- 共显时 `prompt` / `steer` 写入前照常清空输入框：人在 TUI 里写了没发的内容会被清掉（GUI 为主、TUI 里少写草稿，先不处理）
- 第一次查找 transcript 时偶发的文件系统错误会让已有 transcript 之后按实时读，重放历史 block
- 连续两次审批合并成同一段 waiting 的情形只做了逻辑处理（插件报来前一个的 PostToolUse 后重新认定），没有端到端用例
- 同时有几个后台 Subagent 在等审批、且工具输入里都没有可在审批框里比对的内容时，认不出屏幕上是哪个，改发 `dialog`
- 后台 Subagent 的请求留到插件报来它的结果才结束：Subagent 被停掉、没有结果时它一直挂着，会妨碍认定连接前就在等的审批框
- 插话在 Stop 之后自成一轮时，两轮的用量在同一次读 transcript 里到达，可能都算进前一轮
- Workflow Run 偶尔只读到 `journal.jsonl`：`workflow` 事件的 `name` 为 null、`phases` 为空直到结束（端到端偶发，`workflows/<runId>.json` 没被读到）
- 插件装在用户级：每次工具调用都会起一次 `ccrpc hook` 进程（不在 clay Pane 里立即退出），换了 ccrpc 可执行文件的位置要重新 `ccrpc plugin install`
- Subagent Run 文件变短（被替换）时从头重读，会重发其中的 block
- 原样重发的判定只认纯文字提示词：带图片的提示词重发也会报 `branch`；带图片的提示词也不参与 Branch 的分叉判定
- `get_messages` 的第一次要读完整个主文件（连接时就在后台开始），几十 MB 的 transcript 要等几秒
- 普通对话框等待（非审批）里连续出现两个菜单时，第二个 choice 会被吞掉（与 `dialog:` 公告的既有行为相同）
- 分析进行中（最长 60 秒）整个等待都不报告，换成别的认不出的屏幕也要等它结束才降级为 `dialog`
- 审批类工具若以审批之外的 waitingFor 出现、又被学成标题不像审批的 choice，会盖住工具（未见过）

## 维护

- Claude Code 升级后重跑端到端；识别失败时用 `ccrpc debug screen` 拍新布局存进 fixture，更新 recognize.rs 与 probes.md 的版本
