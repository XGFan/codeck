# ccrpc 接管 PTY 尺寸，被接手即只读

ccrpc 不与人共用 Session：人在用的 Session 它不连，人接手了它就停手。最初 ccrpc 作为普通附着端，不发 `Resize`，靠只读打开 Pane 的 tty 读 `TIOCGWINSZ` 得知尺寸，每看一次屏幕都向 Holder 要一次完整 Snapshot。现在改为：附着后发 `Resize` 成为尺寸所有者（缺省 160×48），用 Holder 的 Output 流维护常驻屏幕。ToH 的尺寸归最后操作者，Client 只在把 Pane 切到前台或按键时才抢，所以尺寸所有者就是“谁在用这个 Pane”的信号。ccrpc 订阅 Holder 的尺寸所有者通知：attach 时所有者是另一个在线 Client 就拒绝（`--force` 跳过）；连着时所有者被人拿走，就发 `taken_over`，之后只读，直到调用方重新连接，不抢回尺寸。

## Considered Options

- **保持不发 Resize，由 Holder 向所有附着方广播尺寸变化**：能维持“附着不改 PTY”，但 ccrpc 只能被动接受人或别的 Client 设的尺寸（可能很窄，题目和审批框折行，识别更脆），还要另外判断是否有人在用。
- **看是否有附着方来判断有人在用**：ToH 的 Client 在 Pane 退到后台时仍保持附着，有附着方不等于有人在用。
- **人接手时暂停写入、交还后恢复（接手语义）**：要判断“交还”的时机，并处理两边写入撞车，复杂度和前提不符；现在由调用方重新 attach 来恢复。

## Consequences

- 尺寸所有者通知是 Holder 的新帧，由 Client 在 Attach 里声明订阅才发：toh-proto 的解码器遇到未知帧会报错，不订阅的 Mac、iOS、web 不受影响。
- 机器上已经在跑的旧 Holder 不报尺寸所有者：ccrpc 放行并在 stderr 提示，只靠尺寸变化判断接手。
- 人在 ToH 上把受控 Session 的 Pane 切到前台，会看到按 ccrpc 尺寸排版的画面，直到他的 Client 抢回尺寸后 Claude Code 重绘。
