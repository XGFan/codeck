# CLAUDE.md

`crates/codeck-cli`：命令行 `codeck`，给 AI 查询 Session 与新建 Session（ADR-0013）；用法见 [SKILL.md](SKILL.md)。命令、数据流、跨模块的规则与约束见根目录的 [CLAUDE.md](../../CLAUDE.md)。

- `sessions` / `search` / `show` 经 `Engine::open_read_only`（`Index::open_read_only`）读索引，不起 `Host`：只读打开，不建目录、不建库、不写 `user_version`，版本或数据根不一致时报错而不重建（不然版本不同的 CLI 一运行就把桌面的索引删掉）；没有 `-wal` 时用 `immutable`，免得 SQLite 建 `-wal` / `-shm`。`Engine` 里只有只读模式才会把索引没有的 Session 退回 `scan_sessions` 找文件。
- `turns.rs` 是轮次编号的唯一来源（`show --turn` 与 `search` 共用，在带隐藏节点的 Transcript 上算）；`search` 经 `resolve_jump` 把命中换成显示节点再查编号，落在 Subagent Run 或已回退 Branch 上的写 `turn: null`。
- `wait` 只读进程文件，不碰索引。`new` 走 `ccrpc new`，启动规则与环境同桌面（`codeck_host::ccrpc::{env, launch}`）；以 `!` 开头的提示词要拒绝（ccrpc 会当 shell 命令执行，绕过审批）；`ready` 的 `session_id` 常为 null，要等 `started` 才发提示词。输出类型在这个 crate 里单独定义，不直接用 IPC 类型。
