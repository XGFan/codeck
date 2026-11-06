# TODO

ccrpc 的待办在 [crates/ccrpc/TODO.md](crates/ccrpc/TODO.md)。

## 对话记录的消息类型

- 把对话记录里所有消息类型的界面过一遍（Subagent、Teammate、后台任务通知、Agent 之间的消息、斜杠命令输出、系统注入等），列出哪些要调整，统一设计后再改。
- SendMessage 与收件的 Agent 没有串起来：主线发给 Subagent / Teammate 的 SendMessage 只是一行工具调用，看不出发给了谁、对方何时收到、回了什么；反过来对方的回信也链不回这次 SendMessage。
- Agent 之间的消息（`origin.kind` 为 `peer`）没有专门处理。2026-11-06 扫了本机全部 Session：
  - `<cross-session-message from=… from-name=… from-mode=…>`（另一个 claude 会话经 SendMessage 发来；`origin` 里有 `from`、`name`、`fromMode`、`msg_id`、`body`）：空闲时送达的记成 `isMeta` 的 user 条目，按系统注入默认隐藏（68 条，16 个 Session）；忙时送达的记成 `queued_command` 附件，显示成「你」的气泡（45 条，13 个 Session）。
  - `<agent-message from=…>`（同一会话里的 Subagent 或 Teammate 经 SendMessage 发来，`origin` 带 `senderTaskId`；最早见于 2.1.207）：同样隐藏（123 条，17 个 Session）或显示成「你」的气泡（28 条，12 个 Session）。
  - 两者前面的套话除了 `Another Claude session sent a message:`，还有 `… sent a message while you were working:`。
- 插件发来的消息（`origin.kind` 为 `plugin`，正文以 `The <name> plugin sent a message:` 开头）显示成「你」的气泡（1 条）；channel 消息（`origin.kind` 为 `channel`，`<channel source=…>`）按系统注入隐藏（1 条）。
- 条目类型 `session-owner`（账号与组织 id，53 个 Session）、`dev-mods`（1 个）不在 `known.rs`，进了格式兼容性诊断的「未知」；都是元数据，可登记为已知并隐藏。
- 新的消息类型要靠用户碰到才发现：考虑让格式兼容性诊断也统计「按用户输入处理、但带结构标记」的条目（开头是认不出的标签、`origin.kind` 不是 `human`、或带固定套话），新版本 claude 加了类型时能在诊断页看到。
