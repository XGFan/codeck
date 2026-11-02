# TODO

## 发布前（当前改动收尾）

- 把 toh（`db2656c`、`6d3a1f3`）与 claude-viewer（`1f1ceb3`、`64c04f6`）推到 GitHub；ccrpc 去掉 `Cargo.toml` 的 `[patch]`、`cargo update` 锁定新提交
- ccrpc 建远程仓库并推送
- 本机 `~/.local/bin/toh` 换成带尺寸所有者通知的版本；之前起的旧 Holder 只能靠 PTY 尺寸变化判断接手
- 连接时检查依赖版本：Holder 不报尺寸所有者、Claude Code 不是实测过的版本时，`ready` 带上告警（现在只在 stderr 提示旧 Holder）

## 兼容 Claude Code 升级（下一步，方案已定）

- 跟最新版：记下实测过的 Claude Code 版本，`ready` 与 `ccrpc doctor` 遇到没实测过的版本时告警、照常工作
- `ccrpc doctor`：toh / Holder / claude 版本、配置目录、进程文件、TUI 模式、布局库与分析器配置
- 布局库：识别器改为按布局（见 CONTEXT.md「布局」）识别；内置布局来自实测，认不出的屏幕交给分析器
  - 分析器：ccrpc 自己调 Anthropic 兼容接口，缺省 DeepSeek（复用端到端的 key，可用配置改 base URL / key / 模型）；没配就关闭，行为同现在
  - 发整屏文字和挂起工具的输入
  - 分析完立即生效（不经人确认）；按键前仍做结构检查：审批类必须同时有允许与拒绝、transcript 里有对应的挂起工具，过不了照旧 `unrecognized`
  - 工具审批归为 `permission_request`、提问归为 `question`，其余带编号菜单的发新事件 `choice`（见 CONTEXT.md「选择」），用新命令 `answer_choice` 按选项文字作答
  - 存本机（每个布局一个文件，附当时的屏幕与 Claude Code 版本），另有命令导出进仓库成为内置布局与 fixture
  - 同一屏幕只分析一次，并限制分析频率
  - CLAUDE.md 的硬约束“只认见过的布局”随之改写
- 全屏 TUI（日常配置是 `"tui": "fullscreen"`）：实测输入框、审批框、提问框的布局，内置布局补上；端到端在两种 TUI 下各跑一遍
- 回放探针收进仓库（`ccrpc debug replay <jsonl…>`）：检查实时视图与 `get_messages` 一致、Branch 只落在人发的提示词上

## 协议补全（下一步，方案已定）

- `turn_end` 带出错（API 报错、被中止）、用时、token 用量与估算的 API 金额（cv-core 的 pricing，没有价格的模型为 null）
- `set_mode`：manual / accept edits / plan / auto，按 shift+tab 并核对屏幕上的模式行
- `follow_up` 队列可查看、可撤销
- `reclaim`：接手的 Client 已断开（没有在线的尺寸所有者）时直接取回，重新发 Resize、恢复写入、发 `reclaimed`；对方还在时报错，`force` 跳过
- 后台任务：列出 / 停止后台 Subagent 与 Workflow Run（先实测管理界面）
- 带图片的提示词：`prompt` 带 `images: [Host 上的文件路径]`，以 bracketed paste 写入路径、确认输入框出现 `[Image #n]` 后再写正文；不碰剪贴板（先实测）
- 消息格式写成 JSON Schema

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
- 原样重发的判定只认纯文字提示词：带图片的提示词重发也会报 `branch`
- `get_messages` 每次从头重放主文件，很长的 Session 会慢

## 维护

- Claude Code 升级后重跑端到端；识别失败时用 `ccrpc debug screen` 拍新布局存进 fixture，更新 recognize.rs 与 probes.md 的版本
