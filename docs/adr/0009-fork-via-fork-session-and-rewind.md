# Fork 由 ccrpc 用 `--fork-session` 加回退制造

CoDeck 要能从某个 Session 的某条提示词之前 Fork 出新 Session。Claude Code（2.1.318）交互模式下没有「在指定消息处 Fork」的入口：`--resume <id> --fork-session` 与 `/branch` 都复制整条 Main Line。所以 Fork 由 ccrpc 一条命令完成：在新 Pane 里 `claude --resume <id> --fork-session`，再在新 Session 里回退到那条提示词之前（默认只恢复对话；选了恢复代码、确认页带 ⚠ 覆盖警告时停下让用户确认），最后 `/rename "<标题> (Fork)"`，完成后才报 `ready`。CoDeck 一侧只是 Resume 的变体。

实测的 Claude Code 行为（这套做法依赖它们）：`--fork-session` 起来后、回退或 `/rename` 之前不写任何文件，此时失败只要关掉 Pane 就什么都不留；回退写出的文件只含回退点之前的历史，条目的 uuid 与 Origin Session 相同、**没有 `forkedFrom`**，靠「共享首条消息」的兜底判定认作 Fork；`/rename` 记成本地命令（算用户输入），所以先回退再改名，它落在 Fork 点上，不在 Fork 里留下一段被回退掉的 Branch。

## Considered Options

- **`--resume-session-at <uuid>`**：Claude Code 能在任意 uuid 处截断（包括别的 Branch 上），但它是隐藏参数、只在 print 模式下生效，而 ccrpc 不用 `-p`。代价是 Fork 点只能是 Main Line 上人输入的提示词（回退选择器只列这些）。
- **CoDeck 自己写一份截断的 transcript 再 resume**：违反数据目录只读（ADR-0002）。
- **CoDeck 前端串联 ccrpc 现有的 `new` / `rewind` / `answer_rewind` / `prompt`**：前端要为每一步写失败清理，而「新 Session 还没有文件时按 Origin Session 的 transcript 定位提示词」「输入框里有回填的提示词时发本地命令」这些读屏细节反正得放在 ccrpc；放在 ccrpc 里脚本也能用（ADR-0007 里 ccrpc 是独立的产品）。

## Consequences

- Fork 点之后的代码默认保持现状：Fork 与 Origin Session 共用工作目录，恢复代码会覆盖 Origin Session 正在用的文件。
- Fork 出的 Session 里有一条 `/rename … (Fork)` 的用户输入，计入消息数。
- 依赖 Claude Code 回退选择器与确认页的屏幕格式；它们变了，Fork 与回退一起要跟着改。
