# 接入其他 Agent：agy、codex、pi

CoDeck 读 Claude Code 之外三个 Agent 的对话，接入深度不同：agy 的对话是一等 Session（ADR-0018）；codex、pi 只作为 haa Worker 的对话，在派活 Session 的 Worker 面板里按 id 现读（ADR-0020）。

**以后再接新的 Agent 后端，默认照 codex / pi 的做法**（见下面「接入新的后端」）。agy 是先做的、按「完整接入」（索引、侧栏、Live，预留 Connect）设计的例外；它若以后也改成这种做法，要拆掉的是它的扫描与同步、`Agent::Agy`、侧栏归属与 Live 探测。

## 共同的部分

- **格式只在一个文件里**：`crates/transcript/src/<agent>.rs` 读文件，把原始条目译成 Claude Code 的 `RawEntry`（工具译成 Claude Code 的工具名，交给现有渲染器）；下游的组装、工具渲染、工具详情、图片、⌘F 不分 Agent。codex 与 pi 共用的找文件、切完整行、派生 uuid、造条目在 `foreign.rs`，它不知道任何一边的格式（agy 的 uuid 也用它）。
- **数据目录只读**（ADR-0002），默认目录可用 `CODECK_<AGENT>_ROOT` 换（开发与测试用）。
- **不定价、不进统计**：Cost 写「未记录」。
- **认不出的条目不丢**：照原文显示为认不出的条目。

## 对照

| | agy | codex | pi |
|---|---|---|---|
| 依据 | ADR-0018 | ADR-0020 | ADR-0020 |
| 原始记录 | `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript_full.jsonl` | `~/.codex/sessions/<年>/<月>/<日>/rollout-<时间>-<thread id>.jsonl`，旧的压成 `.jsonl.zst`，也找 `archived_sessions/` | `~/.pi/agent/sessions/<目录>/<时间>_<id>.jsonl` |
| 环境变量 | `CODECK_AGY_ROOT` | `CODECK_CODEX_ROOT` | `CODECK_PI_ROOT` |
| 对话 id | conversation id（`brain/` 下的目录名） | thread id | session id（haa 起的是它预先生成的 `haa-<uuid>`） |
| 是 Session 吗 | 是：扫描、进索引，Agent 记为 `agy` | 不是 | 不是 |
| 什么时候读 | 同步时；文件一变就整份重读（同一个 step 会被重写） | Engine 在索引里找不到这个 id 时按 id 找文件，每次整份重读 | 同 codex |
| 结构 | 一条直链 | 一条直链 | 按 `id` / `parentId` 成树，有 Branch |
| 取哪些内容 | 全部 step；用户 step 只取 `<USER_REQUEST>` 之间 | 只取 codex 界面显示的那一份（`item_completed`），不显示模型那一份（开发者指令、调用工具的 `exec` 脚本） | 全部消息 |
| 工具 | 6 种译成 Bash / Read / Edit / Write / TodoWrite / WebSearch，详情里另留 agy 的原调用（`agyOriginal`），其余按原名 | 命令 → Bash（退出码不为 0 算出错），文件改动 → Write / 带 diff 的 Edit / Delete，MCP → `mcp__<server>__<tool>`，其余按原名 | bash / read / edit / write / grep / find / ls → Bash / Read / Edit（改多处时 MultiEdit）/ Write / Grep / Glob / LS，其余按原名 |
| token | 有（step 上的用量） | 没有 | 有（每条回复的用量） |
| 认不出的条目 | 照原文显示，并进诊断「认不出的消息」 | 照原文显示，不进诊断（不在索引里） | 同 codex |
| 侧栏、列表 | 有；haa Worker 挂在派活的 Session 下 | 没有 | 没有 |
| 搜索 | 按标题、Project 能找到，对话内容不进全文检索 | 找不到 | 找不到 |
| 单独打开（整页） | 有；头部写「agy · 模型」，有「复制 Resume 命令」 | 没有 | 没有 |
| Worker 面板的指标行 | 取索引：`agy · 模型`、消息数、时长、输出 token | 取 haa：`codex · 模型`、Task 个数、Task 时长 | 取 haa：`pi · 模型`、Task 个数、Task 时长 |
| Live / 跟随 | 看哪个 agy 进程开着 presence 锁（状态未知）；文件变化经 watcher 触发同步 | 没有 Live；Worker 在干活时面板每 2 秒重读 | 同 codex |
| 认出是 haa Worker | 猜：workspace 在 `~/.haa` 下，或调过 `mcp_haa_*`；再按 `usage.jsonl` / `workers.json` 的 `sessionId` 连回派活的 Session | 不用认：只从派活 Session 的状态条、Task 卡片进入 | 同 codex |
| 状态条那一行能点 | 对话在 Session 列表里 | 有 `sessionId` | 有 `sessionId` |
| Task 卡片能打开 | 对话在 Session 列表里 | 有 `sessionId`；是哪个后端看 `HaaTask.backend` | 同 codex |
| 旧 Task 记录的 `sessionId` | haa 的 `scripts/backfill-task-session-ids.ts` | haa 的 `scripts/backfill-codex-pi-session-ids.ts` | 同 codex |
| Project | 第一个 workspace；haa worktree 按 worktree 记录归回原仓库 | 没有 | 没有 |
| CLI | `sessions`、`show`；`wait` 不支持 | `show <id>`（同一个按 id 读的回退） | 同 codex |
| 已知限制 | 很快停掉的 ACP 对话可能没有标题、Project 是数据目录 | codex 把一个很长的 thread 接着写进 `…-<thread id>_<id>.jsonl` 时只读第一个文件 | 无 |

## 接入新的后端

照 codex / pi，按顺序：

1. **格式**：新建 `crates/transcript/src/<agent>.rs`（`lib.rs` 里 `pub mod`），只它知道这个后端的格式。
   - 提供 `find_…(root, id)`：先用 `foreign::is_plain_id` 检查 id，只拿它去比对文件名，不拼进路径。
   - 提供 `read(path, id) -> (ParsedChunk, u32)`。
   - 只译界面要显示的那一份；认不出的条目用 `foreign::unknown("<agent>:<type>")`。
   - 测试 `crates/transcript/tests/<agent>.rs`：内联数据，外加一个读 `CODECK_<AGENT>_ROOT` 的 `#[ignore]` 真实数据探针，要求全部读通、没有坏行、工具调用都配上结果。
2. **codeck-core**：
   - `FileSource` 加一项（`compute.rs` 里与 agy 并列、`writer::source_to_db` 都只为穷尽）。
   - `EngineConfig` 加 `<agent>_root`；`paths.rs` 加 `resolve_<agent>_root`（用 `resolve_root_with`）。
   - `session_cache::raw_worker_file` 的查找链加一环，`load_fresh` 加一个分支。
   - `tests/worker_files.rs` 加一例。
3. **codeck-host 与 CLI**：`HostConfig`、`WorkerPaths`、`state::open_engine` 带上这个根；CLI 的 `EngineConfig` 用 `resolve_<agent>_root`；所有构造 `EngineConfig` / `HostConfig` 的测试给临时目录，别读到本机的真实数据。
4. **haa 归属**：`codeck-core/src/haa.rs` 的 `task_backend` 认识新后端的名字；前端 `features/haa/view.ts` 的 `isReadById` 加上它。状态条的行与 Task 卡片据此判断能不能打开。
5. **haa 那边**：`ls --json` 与 `usage.jsonl` 的 Task 记录带上这个后端的会话 id；之前的旧记录照 `scripts/backfill-codex-pi-session-ids.ts` 按 Task 内容补。
6. **mock 与 E2E**：`web/src/mocks/data/haaWorkers.ts` 加一份它的对话，haa 的 E2E 补上「可点」与面板的用例。
7. **文档**：本文的对照表；spec「数据模型」与「已知限制」；README（读哪些目录、环境变量）；根目录与各 crate 的 CLAUDE.md。
