# TODO

ccrpc 的待办在 [crates/ccrpc/TODO.md](../crates/ccrpc/TODO.md)。

## 局域网配对

- 设计画布还没画：桌面「局域网访问」里的「配对新设备」弹层与「已配对的设备」列表、iPhone 的扫码页与外部链接的确认框。按 CLAUDE.md 的画布规则补进对应区域页（3 · 输入区与 Connect 或 1 · 窗口总览，iPhone 页）。

## 对话记录的消息类型

- 把对话记录里所有消息类型的界面过一遍（Subagent、Teammate、后台任务通知、Agent 之间的消息、斜杠命令输出、系统注入等），列出哪些要调整，统一设计后再改。
- SendMessage 与收件的 Agent 没有串起来：主线发给 Subagent / Teammate 的 SendMessage 只是一行工具调用，看不出发给了谁、对方何时收到、回了什么；反过来对方的回信也链不回这次 SendMessage。
- Agent 之间的消息（`<cross-session-message>`、`<agent-message>`）与插件的消息已显示为来源卡片，样式随上面的统一过一遍再定；channel 消息（`origin.kind` 为 `channel`，`<channel source=…>`，本机只见过 1 条测试消息）仍按系统注入隐藏，会出现在格式兼容性诊断的「认不出的消息」里。
- 条目类型 `session-owner`（账号与组织 id，53 个 Session）、`dev-mods`（1 个）不在 `known.rs`，进了格式兼容性诊断的「未知」；都是元数据，可登记为已知并隐藏。
