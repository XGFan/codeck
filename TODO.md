# TODO

## 收尾：输入输出机制优化（ccrpc、toh、claude-viewer 一起改，已落地）

- 稳定后把 toh（`db2656c` 起）与 claude-viewer（`1f1ceb3` 起）推到 GitHub，ccrpc 去掉 `Cargo.toml` 的 `[patch]`、`cargo update` 锁定新提交
- 本机日常用的 `~/.local/bin/toh` 换成带尺寸所有者通知的新版本；在那之前接入旧 Holder 时只靠 PTY 尺寸变化判断接手
- 行读取与 transcript 路径解析仍是 ccrpc 自己的：cv-core 的 `read_entries` 会把 `queued_command` 附件改写成用户消息（ccrpc 要靠它区分插话），也没有按 cwd 求 transcript 路径的函数
- Main Line 不做增量：只在一个非 assistant、非工具结果的条目没接在上一条后面时，用 cv-core 整份重算一次
- 待确认：ToH Client 会不会把 `ccrpc new` 刚建的 Pane 自动切到前台并抢尺寸（会的话 ccrpc 一连上就被判为接手）
- 待确认：后台挂着的 Mac / web Client 上次操作过的 Pane 仍算“有人在用”，`attach` 要 `--force`；是否需要按最近一次输入时间区分
- 待确认：按键间隔现在常是 50ms（忙时 spinner 也算屏幕有反应），端到端目前都过，换慢机器或更长菜单时留意
- Subagent Run 文件变短（被替换）时从头重读，会重发其中的 block

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
