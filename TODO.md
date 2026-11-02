# TODO

## 发布前（当前改动收尾）

- 把 toh（`db2656c`、`6d3a1f3`）与 claude-viewer（`1f1ceb3`、`64c04f6`）推到 GitHub；ccrpc 去掉 `Cargo.toml` 的 `[patch]`、`cargo update` 锁定新提交
- ccrpc 建远程仓库并推送
- 本机 `~/.local/bin/toh` 换成带尺寸所有者通知的版本；之前起的旧 Holder 只能靠 PTY 尺寸变化判断接手

## 兼容升级与协议补全（已落地，后续）

- 学到的布局只在本机：常见界面学到后用 `ccrpc layouts export .` 导出成内置布局，提交前检查其中的整屏文字
- 分析器只处理“等待作答的对话框”；输入框、模式行、后台任务面板仍是代码里的识别，Claude Code 改了这些界面要改 recognize.rs
- `stop_task` 停 Subagent 只按面板上的描述匹配，没有端到端用例（目前只测了后台 shell）
- 被 ccrpc 停掉的后台任务先记为 `stopped`，Claude Code 的结束通知要等主线下一轮才写进 transcript
- 全屏 TUI 下拍到的输入框、审批框、提问框、后台面板与默认 TUI 一致；计划审批只在默认 TUI 下实测

## 形态

- 常驻进程 + unix socket（一条连接管多个 Session，沿用同一套 JSONL 消息）
- 同一 Session 连多个 ccrpc
- 调用方 SDK（Rust、TypeScript / Python）与示例
- 日志：`--log FILE` 记下状态判定、屏幕识别、按键，排查 `unrecognized` 与超时用
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

- /clear 后约 1.3s 内（新 sessionId 被察觉之前）发出的 prompt，其 block 落在“历史”里不发出（prompt 本身照常成功）
- 第一次查找 transcript 时偶发的文件系统错误会让已有 transcript 之后按实时读，重放历史 block
- 同一个审批框期间进程文件若 waiting→busy→waiting 抖动，该请求会降级为 dialog，需用 send_keys 作答
- 连续两次审批合并成同一段 waiting 的情形只做了逻辑处理，没有端到端用例
- Subagent Run 文件变短（被替换）时从头重读，会重发其中的 block
- 原样重发的判定只认纯文字提示词：带图片的提示词重发也会报 `branch`；带图片的提示词也不参与 Branch 的分叉判定
- `get_messages` 每次从头重放主文件，很长的 Session 会慢
- 普通对话框等待（非审批）里连续出现两个菜单时，第二个 choice 会被吞掉（与 `dialog:` 公告的既有行为相同）
- 分析进行中（最长 60 秒）整个等待都不报告，换成别的认不出的屏幕也要等它结束才降级为 `dialog`
- 审批类工具若以审批之外的 waitingFor 出现、又被学成标题不像审批的 choice，会盖住工具（未见过）

## 维护

- Claude Code 升级后重跑端到端；识别失败时用 `ccrpc debug screen` 拍新布局存进 fixture，更新 recognize.rs 与 probes.md 的版本
