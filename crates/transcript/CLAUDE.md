# CLAUDE.md

`crates/transcript`：解析库，读 Claude Code 的数据（transcript、进程文件、价格）并组装成显示模型；无 SQLite、无 tauri，codeck-core 与 ccrpc 都用它。命令、数据流、跨模块的规则与约束见根目录的 [CLAUDE.md](../../CLAUDE.md)。

- `crates/transcript` 放解析层：`raw`、`parse`、`known`、`assemble/*`、`model/*`（组装用的类型）、`live`（进程文件）、`layout`（Session 目录里 agent 与 sidecar 文件的位置，codeck-core 的扫描与 ccrpc 共用）、`pricing`。codeck-core 直接写 `transcript::…`。解析层的集成测试（parse / merge / branch / assemble / code_mode，共用的辅助函数在 `tests/support/`）在 `crates/transcript/tests/`，fixture 用 codeck-core 的，经 `#[path]` 引用 `crates/codeck-core/tests/common/fixtures.rs`（该文件不能依赖 codeck-core）。解析层的错误是 `TranscriptError`，codeck-core 按变体映射到 `CoreError`。
- Code Mode（Claude Code 的 `CLAUDE_CODE_EXPERIMENTAL_CODE_MODE`，规则见 spec「Code Mode」，各部分在哪、哪些可删见下面「Code Mode」一节）：解析与对原始格式的判断只在 `assemble/code_mode.rs`（事件解析、按路径把虚拟消息归给代码）；模型里代码仍是一个 `REPL` 的 `ToolCall`（`code` 带 `callCount` 与返回值），它调用的工具是同一条消息里紧随其后的平级 `ToolCall`（`viaCode` 指回代码），前端、⌘F、CLI 都按这个平的序列走，不要再嵌套。
- Teammate 与其他 Agent、插件转发来的消息（`<teammate-message>`、`<cross-session-message>`、`<agent-message>`）只在后端拆：`prompt::relayed_messages` 给出 `UserPrompt.relayed`（发送方、颜色、summary、正文、一行预览、Teammate 的 JSON 消息说的事），前端 `features/transcript/teammate.ts` 只管排版，不读标签。两处例外：mock 数据按记录的原文写，`src/mocks/data/relayed.ts` 替后端拆（改格式时对照 Rust 的测试）；Connect 的排队提示拿到的是 ccrpc 给的原文，`connect/composer/queue.ts` 只读开头标签的发送方与 summary。
