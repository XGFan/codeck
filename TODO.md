# TODO

## 下一步：使用方（协议之上的子项目，各自单独设计）

- 主控 Claude Code 调度 Claude Code Worker（类 paaa）：派 Task、取 Result、Ask / Approval 转给主控、插话、中止；事件以 monitor 推给主控；人可随时在 ToH 上接手
- ToH Client 的结构化操作：Session 状态（忙 / 闲 / 等审批）、审批卡片、发提示词、长任务完成推送（对应 ToH TODO 的「长任务完成通知推送」）
- 远程调用：经 ToH daemon 暴露，复用它的令牌、配对与 tailnet

## 协议与形态

- 人与调用方同时写入的冲突处理（接手语义：有人附着或在打字时暂停写入，交还后通知调用方）
- 同一 Session 连多个 ccrpc
- 常驻进程 + unix socket 形态（一条连接管多个 Session，沿用同一套 JSONL 消息）
- Workflow Run 内部结构事件（阶段、各 Subagent Run 的归属）

## 已知限制（MVP 遗留）

- 多选题的 Other（自由文本）作答：先实测布局与按键，目前返回 unsupported
- rewind / 编辑重发产生的 Branch：block 与 get_messages 目前按写入顺序，不区分 Main Line（可用 cv-core 的 Main Line 规则）
- /clear 后约 1.3s 内（新 sessionId 被察觉之前）发出的 prompt，其 block 落在“历史”里不发出（prompt 本身照常成功）
- 第一次查找 transcript 时偶发的文件系统错误会让已有 transcript 之后按实时读，重放历史 block
- 同一个审批框期间进程文件若 waiting→busy→waiting 抖动，该请求会降级为 dialog，需用 send_keys 作答
- 连续两次审批合并成同一段 waiting 的情形只做了逻辑处理，没有端到端用例

## 维护

- 建远程仓库并推送（目前只有本地提交；cv-core、toh-proto 的 git 依赖已指向 GitHub）
- Claude Code 升级后重跑端到端；识别失败时用 `ccrpc debug screen` 拍新布局存进 fixture，更新 recognize.rs 与 probes.md 的版本

## 相关项目

- Claude Viewer 的 Live 标记区分 waiting（进程文件 status=waiting + waitingFor），目前它把 busy 以外都当 idle
