# 接入其他 Agent：agy、codex、pi

CoDeck 读 Claude Code 之外三个 Agent 的对话，只为在派活的 Session 里看 haa 派出的 Worker 做了什么：三者一样按 id 现读（ADR-0020、ADR-0021），不是 Session，只从 haa 状态条那一行与 Task 卡片打开右侧面板。不是 haa 起的对话不显示。以后再接新的 Agent 后端也照此（见下面「接入新的后端」）。

## 共同的部分

- **格式只在一个文件里**：`crates/transcript/src/<agent>.rs` 找文件、读文件，把原始条目译成 Claude Code 的 `RawEntry`（工具译成 Claude Code 的工具名，交给现有渲染器）；下游的组装、工具渲染、工具详情、图片、⌘F 不分 Agent。找文件、切完整行、派生 uuid、造条目的共用部分在 `foreign.rs`，它不知道任何一边的格式。
- **只按 id 读**：不扫描、不进索引、列表、侧栏、搜索与统计。`session_cache` 在索引里找不到一个 id 时，依次按 id 找 agy、codex、pi 的文件（`FileSource::Agy` / `Codex` / `Pi`），每次整份重读；没有 `get_session` 的详情，也没有整页视图。
- **入口与面板**：haa `ls` 的行或 `usage.jsonl` 的 Task 记录有 `sessionId`、且后端是这三者之一就能打开（行看模型名的 `<backend>/` 前缀，Task 卡片看 `HaaTask.backend`：模型名前缀，否则 haa 记的版本字段，都没有的是 haa 记版本之前的 pi）。面板的指标行都取 haa 的：`<Agent> · 模型`、Task 个数、Task 时长之和、Cost 未记录。
- **实时跟随**：Host 的 watcher 监听三个后端的会话目录，从变化的路径认出对话 id（`agy::session_of_transcript`、`codex::thread_of_path`、`pi::session_of_path`），发 `worker-files-changed-event`（只给桌面窗口），打开着这个对话的面板随之重读；codex 跑着时一直开着 rollout，FSEvents 对开着的文件要到关闭时才报修改，所以 Host 另外每秒按大小与修改时间检查 codex 最新两天目录里的 rollout（`codex::recent_rollouts`）；Worker 在干活时面板停在底部，底部写 haa `ls` 的 `activity`（「正在做：…」）。不 Connect，也不看后端进程。
- **数据目录只读**（ADR-0002），默认目录可用 `CODECK_<AGENT>_ROOT` 换（开发与测试用）。
- **不定价、不进统计**：Cost 写「未记录」。
- **认不出的条目不丢**：照原文显示为认不出的条目；不在索引里，所以不进诊断。
- **CLI**：`codeck show <id>` 经同一个按 id 读的回退读出对话；`sessions`、`search` 里没有它们。

## 对照

| | agy | codex | pi |
|---|---|---|---|
| 原始记录 | `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript_full.jsonl` | `~/.codex/sessions/<年>/<月>/<日>/rollout-<时间>-<thread id>.jsonl`，旧的压成 `.jsonl.zst`，也找 `archived_sessions/` | `~/.pi/agent/sessions/<目录>/<时间>_<id>.jsonl` |
| 环境变量 | `CODECK_AGY_ROOT` | `CODECK_CODEX_ROOT` | `CODECK_PI_ROOT` |
| 对话 id | conversation id（`brain/` 下的目录名） | thread id | session id（haa 起的是它预先生成的 `haa-<uuid>`） |
| 找文件 | `agy::find_transcript`（路径固定） | `codex::find_rollout`（按日期目录从新到旧找） | `pi::find_session`（逐个目录找） |
| 结构 | 一条直链；同一个 step 下标再出现时以后一行为准 | 一条直链 | 按 `id` / `parentId` 成树，有 Branch |
| 取哪些内容 | 全部 step；用户 step 只取 `<USER_REQUEST>` 之间 | 只取 codex 界面显示的那一份（`item_completed`），不显示模型那一份（开发者指令、调用工具的 `exec` 脚本） | 全部消息 |
| 工具 | 6 种译成 Bash / Read / Edit / Write / TodoWrite / WebSearch，详情里另留 agy 的原调用（`agyOriginal`），其余按原名 | 命令 → Bash（退出码不为 0 算出错），文件改动 → Write / 带 diff 的 Edit / Delete，MCP → `mcp__<server>__<tool>`，其余按原名 | bash / read / edit / write / grep / find / ls → Bash / Read / Edit（改多处时 MultiEdit）/ Write / Grep / Glob / LS，其余按原名 |
| token | 有（step 上的用量） | 没有 | 有（每条回复的用量） |
| 什么时候写文件（跟随看得到什么） | 每个 step 都写，没跑完的命令先写成 `RUNNING`，之后追加一行覆盖 | 只写已完成的 item：命令跑完、回复写完才出现；文件一直开着，靠每秒检查大小与修改时间发现 | 每条消息结束时写：工具调用在执行前出现，结果在跑完时出现 |
| 旧 Task 记录的 `sessionId` | haa 的 `scripts/backfill-task-session-ids.ts` | haa 的 `scripts/backfill-codex-pi-session-ids.ts` | 同 codex |
| 已知限制 | 工具结果不带调用 id，按顺序对上前一条回复的调用 | 一个很长的 thread 接着写进 `…-<thread id>_<id>.jsonl` 时只读第一个文件 | 无 |

## 接入新的后端

按顺序：

1. **格式**：新建 `crates/transcript/src/<agent>.rs`（`lib.rs` 里 `pub mod`），只它知道这个后端的格式。
   - 提供 `find_…(root, id)`：先用 `foreign::is_plain_id` 检查 id，只拿它去比对文件名，不拼进路径。
   - 提供 `read(path, id) -> (ParsedChunk, u32)`。
   - 提供 `…_of_path(root, path) -> Option<String>`：watcher 拿它从变化的路径认出对话 id。
   - 只译界面要显示的那一份；认不出的条目用 `foreign::unknown("<agent>:<type>")`。
   - 测试 `crates/transcript/tests/<agent>.rs`：内联数据，外加一个读 `CODECK_<AGENT>_ROOT` 的 `#[ignore]` 真实数据探针，要求全部读通、没有坏行、工具调用都配上结果。
2. **codeck-core**：
   - `FileSource` 加一项（`session_cache::load_fresh` 加读它的分支；别处的 match 只为穷尽）。
   - `EngineConfig` 加 `<agent>_root`；`paths.rs` 加 `resolve_<agent>_root`（用 `resolve_root_with`）。
   - `session_cache::raw_worker_file` 的查找链加一环。
   - `tests/worker_files.rs` 加一例。
3. **codeck-host 与 CLI**：`HostConfig`、`WorkerPaths`、`state::open_engine` 带上这个根；`watcher::WorkerRoots` 加上它，`watcher::start_workers` 监听它的会话目录，`WorkerMapper::worker_of` 加一环（`watcher.rs` 的测试加一例）；CLI 的 `EngineConfig` 用 `resolve_<agent>_root`；所有构造 `EngineConfig` / `HostConfig` 的测试给临时目录，别读到本机的真实数据。
4. **haa 归属**：`codeck-core/src/haa.rs` 的 `task_backend` 认识新后端的名字；前端 `features/haa/view.ts` 的 `isReadById` 加上它。状态条的行与 Task 卡片据此判断能不能打开。
5. **haa 那边**：`ls --json` 与 `usage.jsonl` 的 Task 记录带上这个后端的会话 id；之前的旧记录照 `scripts/backfill-codex-pi-session-ids.ts` 按 Task 内容补。
6. **mock 与 E2E**：`web/src/mocks/data/haaWorkers.ts` 加一份它的对话，haa 的 E2E 补上「可点」与面板的用例。
7. **文档**：本文的对照表；spec「数据模型」与「已知限制」；README（读哪些目录、环境变量）；根目录与各 crate 的 CLAUDE.md。
