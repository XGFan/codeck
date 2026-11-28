# CLAUDE.md

`crates/codeck-core`：索引、搜索、统计与诊断；纯 Rust，无 tauri 依赖。解析层在 `crates/transcript`（见它的 CLAUDE.md），这里直接写 `transcript::…`。命令、数据流、跨模块的规则与约束见根目录的 [CLAUDE.md](../../CLAUDE.md)。

- `engine/` 的 `Engine` 是 codeck-host 调用的**唯一入口**，全部是阻塞调用（门面 API 在 `engine/mod.rs`）。它负责：索引同步（`engine/sync/`：stat 遍历（FS 批次的路径都落在某些 Session 里时只 stat 这些 Session，启动、Rescan 与其他情况全扫）→ 与 `files` 表比对（`plan.rs`）→ rayon 并行轻解析（`compute.rs`）→ 按批事务写入（`write.rs`）→ 重算受影响的 Session；追加只读 `[parsed_offset, EOF)`，文件变小/被替换则整 Session 重解析）；以及显示用的小 LRU（`engine/session_cache.rs`，按 source/sidecar 的 path+size+mtime 哈希校验）。
- 索引是可丢弃的镜像（ADR-0003）：不写迁移，schema 变化就把 `index/schema.rs` 的 `SCHEMA_VERSION` +1，启动时自动删库重建。
- 工具输出不进 FTS，也不提供搜索（ADR-0004）；⌘F 会话内查找仍覆盖工具输出（`assemble/find.rs`）。
- Cost：`pricing.rs` 按 ccusage 的算法给单条消息定价，通过 SQLite 函数 `cv_cost` 写入 `messages.cost`。`writer::rebill` 实现去重：同一 `message.id` 出现在多个 Session 时只把最早 Session 那份标为 `billed`，Session 的 token / 金额列和统计只累加 billed 的消息。每次同步结束时，对变化的 Session 及与它们共享消息的 Session 重算（`engine/sync/write.rs` 的 `rebill_changed`）。价格表在 `Engine` 里，联网下载在 codeck-host（`prices.rs`，ADR-0005）。
- haa 的 agy / codex / pi Worker（ADR-0020、ADR-0021）：`EngineConfig` 带 `agy_root`、`codex_root`、`pi_root` 与 `haa_home`，都不进索引 `meta`；`session_cache` 的 `session_ctx` 在索引（与只读模式的补扫）都没有这个 id 时调 `raw_worker_file`，按 id 依次经 `transcript::agy::find_transcript` / `codex::find_rollout` / `pi::find_session` 找到文件（`FileSource::Agy` / `Codex` / `Pi`），`load_one` 总是整份重读，没有 sidecar。同步永远看不到这三种文件（别处对 `FileSource` 的 match 只为穷尽）。Task 的后端（`HaaTask.backend`，前端据此决定 Task 卡片能不能打开）由 `haa.rs` 的 `task_backend` 推出：模型名的 `<backend>/` 前缀，否则看 haa 记的版本字段，都没有的是记版本之前的 pi。测试在 `tests/worker_files.rs`。
- 统计（`stats.rs`）：「请求」是计费且带 token 的 assistant 消息（`billed_tokens_sql`，含 Subagent）。`daily`（按天按模型）总给；`hourly` 只在范围上下界都有且不超过 8 天时给；`hourly_models`（每小时按模型）与 `sessions`（各 Session 的 Cost 与时段，相邻请求相隔不超过 10 分钟连成一段）只在不超过 26 小时（单日）时给。下钻（`Drill::Day` / `Model`，`index/reader.rs`）与统计同一口径；被接替的 Session 留下的计费行，下钻与 `sessions` 都沿 `continued_in_id` 归到可见的续接后继。
