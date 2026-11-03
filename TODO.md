# TODO

## 发布前（当前改动收尾）

- 把 toh（`db2656c`、`6d3a1f3`）与 claude-viewer（`1f1ceb3`、`64c04f6`）推到 GitHub；ccrpc 去掉 `Cargo.toml` 的 `[patch]`、`cargo update` 锁定新提交
- ccrpc 建远程仓库并推送
- 本机 `~/.local/bin/toh` 换成带尺寸所有者通知的版本；之前起的旧 Holder 只能靠 PTY 尺寸变化判断接手

## 兼容升级与协议补全（已落地，后续）

- ccrpc 插件：StopFailure（API 报错结束一轮）、PostToolUseFailure / PermissionDenied 结束请求、hook 已超时后回落按键、`allow_always` 经插件回填，都没有端到端用例；Claude Code 升级后先重跑 probes.md 的 hooks 一节（hook 输入字段、阻塞时界面是否照常显示）
- 学到的布局只在本机：常见界面学到后用 `ccrpc layouts export .` 导出成内置布局，提交前检查其中的整屏文字
- 分析器只处理“等待作答的对话框”；输入框、模式行、后台任务面板仍是代码里的识别，Claude Code 改了这些界面要改 recognize.rs
- `stop_task` 停 Subagent 只按面板上的描述匹配，没有端到端用例（目前只测了后台 shell）
- 被 ccrpc 停掉的后台任务先记为 `stopped`，Claude Code 的结束通知要等主线下一轮才写进 transcript
- 全屏 TUI 下拍到的输入框、审批框、提问框、后台面板与默认 TUI 一致；计划审批只在默认 TUI 下实测

## GUI MVP 要的改动（只在本机用，不发布）

GUI 另起一个 Tauri 应用，内容只来自 ccrpc（`get_messages` + `block` + `branch`），每个运行中的 clay Claude Code 各连一个 `ccrpc attach --co-display`；TUI 同时在 Mac Client 里显示，认不出的界面在 TUI 里手动处理。

- 共显（过渡期，GUI 成熟后去掉）：`new` / `attach` 加 `--co-display`，不发 Resize、不判接手，屏幕跟随 PTY 尺寸（复用被接手后的跟随逻辑）；输入框照旧先清空
- `get_state` 加 `mode`（模式行为准，认不出用 transcript 的 `permission-mode`）、`model`（主线最后一条 assistant 的 model）、`context_tokens`（它的 input + cache_read + cache_creation）；三项变化时推送一条新事件
- `exit` 命令：idle 时输入 `/exit`，等 claude 进程消失；否则 `not_idle`
- `ccrpc ls --json`
- 日志：`--log FILE` 记下状态判定、屏幕识别、按键，排查 `unrecognized` 与超时用
- 实测：`claude --resume <id>` 是否换 sessionId；`ccrpc new` 的 Pane 在 claude 退出后是否随之结束
- 之后：进度事件（屏幕 spinner 行，去掉共显前做）、`get_messages` 带上 Subagent Run 的历史

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

- ToH Client 会不会把 `ccrpc new` 刚建的 Pane 自动切到前台并抢尺寸（会的话 ccrpc 一连上就被判为接手）
- 后台挂着的 Mac / web Client 上次操作过的 Pane 仍算“有人在用”，`attach` 要 `--force`；是否改为按最近一次输入时间判断（需要 Holder 报告）
- 按键间隔现在常是 50ms（忙时 spinner 也算屏幕有反应），端到端都过；换慢机器或更长菜单时留意

## 已知限制

- 第一次查找 transcript 时偶发的文件系统错误会让已有 transcript 之后按实时读，重放历史 block
- 连续两次审批合并成同一段 waiting 的情形只做了逻辑处理（插件报来前一个的 PostToolUse 后重新认定），没有端到端用例
- 同时有几个后台 Subagent 在等审批、且工具输入里都没有可在审批框里比对的内容时，认不出屏幕上是哪个，改发 `dialog`
- 后台 Subagent 的请求留到插件报来它的结果才结束：Subagent 被停掉、没有结果时它一直挂着，会妨碍认定连接前就在等的审批框
- 未实测：插话在 Stop 之后才送达、自成一轮时是否再触发 UserPromptSubmit（不触发则这一轮没有 `turn_end`）；经插件的 `allow_always` 与 TUI 第 2 项的效果是否一致
- Workflow Run 偶尔只读到 `journal.jsonl`：`workflow` 事件的 `name` 为 null、`phases` 为空直到结束（端到端偶发，`workflows/<runId>.json` 没被读到）
- 插件装在用户级：每次工具调用都会起一次 `ccrpc hook` 进程（不在 clay Pane 里立即退出），换了 ccrpc 可执行文件的位置要重新 `ccrpc plugin install`
- Subagent Run 文件变短（被替换）时从头重读，会重发其中的 block
- 原样重发的判定只认纯文字提示词：带图片的提示词重发也会报 `branch`；带图片的提示词也不参与 Branch 的分叉判定
- `get_messages` 每次从头重放主文件，很长的 Session 会慢
- 普通对话框等待（非审批）里连续出现两个菜单时，第二个 choice 会被吞掉（与 `dialog:` 公告的既有行为相同）
- 分析进行中（最长 60 秒）整个等待都不报告，换成别的认不出的屏幕也要等它结束才降级为 `dialog`
- 审批类工具若以审批之外的 waitingFor 出现、又被学成标题不像审批的 choice，会盖住工具（未见过）

## 维护

- Claude Code 升级后重跑端到端；识别失败时用 `ccrpc debug screen` 拍新布局存进 fixture，更新 recognize.rs 与 probes.md 的版本
