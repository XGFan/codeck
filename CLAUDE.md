# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

CoDeck：macOS 桌面应用（Tauri 2 + React 19），搜索和阅读 `~/.claude` 下 Claude Code 的历史 Session，并经 ccrpc 驾驶 ToH clay 里运行中的 Session（ADR-0007）。显示名写 CoDeck，代码、包名、路径与环境变量里写小写 `codeck` / `CODECK_`。

## 仓库布局（monorepo）

| 目录 | 内容 | 说明 |
|---|---|---|
| `crates/transcript` | 解析库：读 Claude Code 的数据（transcript、进程文件、价格），组装成显示模型；无 SQLite、无 tauri | 本文件「架构」 |
| `crates/codeck-core`、`src-tauri`、`src` | CoDeck 本体 | 本文件 |
| `crates/ccrpc` | ccrpc：以 JSONL 驱动 ToH clay 里的 Claude Code | 见 [crates/ccrpc/CLAUDE.md](crates/ccrpc/CLAUDE.md) |

一个 Cargo workspace（`cargo test --workspace` 覆盖全部 crate）；前端只有根目录的 viewer。原 cc-gui 已并入 viewer 的驾驶功能（ADR-0007）。驾驶 E2E 的 wire fixture 在 `src/mocks/data/drive/`，`tools.jsonl` 用 `scripts/record-drive-fixtures.sh` 从 ccrpc 的 mock 端到端测试重录。

- 术语以 [CONTEXT.md](CONTEXT.md) 为准（Session / Project / Live Session / Subagent Run / Workflow Run / Branch / Main Line / Fork / Teammate Message），代码、注释、文案都用这些词，避开其中列出的 _Avoid_ 词。
- 行为规格见 [docs/spec.md](docs/spec.md)（计数口径、用户输入判定、Main Line 规则等都在这里），关键取舍见 [docs/adr/](docs/adr/)。改动对外行为时同步更新 spec / README / CONTEXT。

## 常用命令

```sh
pnpm install
pnpm tauri dev                 # 启动真实应用（读取真实数据目录）
pnpm dev:mock                  # 只起前端 + 内置 mock 数据，http://localhost:1420

cargo test --workspace
cargo test -p codeck-core --test parse                 # 单个集成测试文件（tests/*.rs）
cargo test -p codeck-core --test branch <name>         # 单个用例
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                # Rust 格式化（默认 rustfmt 配置）；提交前必须是格式化过的
scripts/install-git-hooks.sh   # 装 pre-commit 钩子：提交含 .rs 时跑 cargo fmt --check，没格式化就拦下

pnpm typecheck && pnpm test                        # tsc + vitest（src/**/*.test.ts）
pnpm vitest run src/lib/diff.test.ts               # 单个前端单测
pnpm exec playwright install webkit                # 首次跑 E2E 前
pnpm test:e2e                                      # Playwright，WebKit，mock 模式
pnpm test:e2e e2e/m3-search.spec.ts -g "<title>"   # 单个 E2E
E2E_PORT=1431 pnpm test:e2e                        # 并行跑多份时各用一个端口（默认 1421）

pnpm bindings                  # 改了 IPC 类型/命令后重新生成 src/ipc/bindings.ts
curl -sSf https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json \
  | cargo run -p transcript --example update_pricing   # 刷新内置价格快照
pnpm tauri build --bundles app --target aarch64-apple-darwin
```

真实数据探针（`#[ignore]`，只读，索引写临时目录）：`CODECK_REAL_ROOT=~/.claude cargo test -p codeck-core --release --test assemble -- --ignored --nocapture`、`CODECK_BENCH_ROOT=~/.claude cargo test -p codeck-core --release --test search -- --ignored --nocapture`。

## 设计稿

界面设计统一放在 Claude Design 画布「CoDeck 设计」：https://claude.ai/artifact/M5J8o7P5DFasg3dYsi2dxs ，不另开画布，只放可编辑的原型图（不放截图）。

- 新设计先在画布上出 2–3 个可编辑画板（按主题建页），选定的标题前标「✓ 采用」，再实现；实现后删掉没采用的方案。
- 之后改了界面，把对应主题里已采用的原型图（及页面上的说明便签）一起改到和实现一致。

## 架构

三层，数据单向流动：`crates/transcript`（解析）→ `crates/codeck-core`（索引、搜索、统计；纯 Rust，无 tauri 依赖）→ `src-tauri`（外壳）→ `src`（React）。ccrpc 也依赖 `crates/transcript`，不依赖 codeck-core。

### transcript 与 codeck-core

- `crates/transcript` 放解析层：`raw`、`parse`、`known`、`assemble/*`、`model/*`、`live`（进程文件）、`pricing`。codeck-core 以原路径 `pub use` 重新导出它们（`codeck_core::model::…`、`crate::assemble::…` 照旧可用）。解析层的错误是 `TranscriptError`，codeck-core 按变体映射到 `CoreError`。

- `engine.rs` 的 `Engine` 是 src-tauri 调用的**唯一入口**，全部是阻塞调用。它负责：索引同步（stat 遍历 → 与 `files` 表比对 → rayon 并行轻解析 → 按批事务写入 → 重算受影响的 Session；追加只读 `[parsed_offset, EOF)`，文件变小/被替换则整 Session 重解析）；以及显示用的小 LRU（按 source/sidecar 的 path+size+mtime 哈希校验）。
- 管线：`scan`（遍历数据根、按 realpath/inode 去重）→ `raw`（宽容的 serde 结构，字段全可选）→ `parse`（按行解析，坏行跳过并计数）→ `assemble/*`（把事件日志组装成节点：parentUuid 树、Branch/Main Line、compact 接续、Fork、Subagent/Workflow 关联、工具摘要、⌘F 查找、搜索跳转）→ `index/*`（SQLite + FTS5 trigram）→ `search` / `stats` / `diag`。`known.rs` 列出已知的条目类型/内容块/工具名，未知的进诊断而不是丢弃。
- `model/*`（在 `crates/transcript`）是 IPC 类型的**唯一来源**（specta 导出）。改这里或 `src-tauri/src/commands.rs` 的签名后必须 `pnpm bindings`；`src/ipc/bindings.ts` 是生成文件，勿手改。
- 索引是可丢弃的镜像（ADR-0003）：不写迁移，schema 变化就把 `index/schema.rs` 的 `SCHEMA_VERSION` +1，启动时自动删库重建。
- 工具输出不进 FTS，也不提供搜索（ADR-0004）；⌘F 会话内查找仍覆盖工具输出（`assemble/find.rs`）。
- Cost：`pricing.rs` 按 ccusage 的算法给单条消息定价，通过 SQLite 函数 `cv_cost` 写入 `messages.cost`。`writer::rebill` 实现去重：同一 `message.id` 出现在多个 Session 时只把最早 Session 那份标为 `billed`，Session 的 token / 金额列和统计只累加 billed 的消息。每次同步结束时，对变化的 Session 及与它们共享消息的 Session 重算（`engine::rebill_changed`）。价格表在 `Engine` 里，联网下载在 src-tauri（`prices.rs`，ADR-0005）。

### src-tauri

- `commands.rs`：每个命令都是 async，通过 `with_engine` 把 Engine 调用丢到 blocking pool，错误转成 `AppError`（`message` 是面向用户的中文文案）。新增命令要同时登记到 `lib.rs` 的 `specta_builder()`。
- `worker.rs`：单个后台 worker 通过 channel 消费 `Job`（FullScan / Fs 批次 / TextBacklog / Rebuild / SetRoot / PollLive / Rescan）。全文索引积压 `TextBacklog` 在空闲时运行，任何其他任务入队都会让它在当前事务提交后让位——改调度逻辑时注意别造成互相重新入队的活锁（曾出现过）。
- `prices.rs`：下载 LiteLLM 价格表；worker 的 `RefreshPrices` 在独立线程里下载，结果以 `ApplyPrices` 交回 worker 写库，价格有变化时发 `PricesChanged` 事件。
- `watcher.rs`（FSEvents，debounce 后发 `Fs` 任务）、`state.rs`（持有可整体替换的 Engine，切换数据根时 swap）、`settings.rs`（数据根覆盖，存于 Application Support）、`events.rs`（IndexStatus / SessionsChanged / LiveChanged / PricesChanged，以及驾驶的 DriveLine / DriveExit 事件）。
- `drive/`：驾驶（ADR-0007）。每个连接一个 `ccrpc attach|new --co-display` 子进程，stdout 逐行原样转发为 `DriveLine` 事件（协议解析与状态都在前端 `features/drive`），三层日志在 `~/Library/Logs/dev.joy.codeck/drive/`。状态在第一个 drive 命令时才初始化（要跑一次登录 shell 取环境），不要挪到启动流程里。对话内容不走 ccrpc，仍由索引与 Live 跟随提供。

### src（前端）

- `src/ipc/`：`Api` 接口（`api.ts`）有两套实现——`real.ts` 包装生成的 bindings，`mock.ts` 用 `src/mocks/data/*` 的内存数据。`VITE_IPC=mock`（`.env.mock`，即 `pnpm dev:mock`）切到 mock。**E2E 全部跑在 mock 上**（tauri-driver 不支持 macOS），所以新增/修改 IPC 能力时要同步 `mock.ts` 和 mock 数据。
- `src/queries/`：TanStack Query hooks 与 `queryKeys`；`events.ts` 订阅后端事件并做节流失效（实时追随、统计刷新）。
- `src/state/ui.ts`：zustand 的 UI 状态；按 Session 的状态（scope、branchChoices、panelStack、⌘F、展开状态等）在切换 Session 时重置；栏的收起状态（`collapsed`）是全局的，存 localStorage。
- 滚动条：`index.css` 自定义 `::-webkit-scrollbar`，`lib/scrollbars.ts` 在滚动时给元素写内联 `--scroll-thumb` 让滑块显示。WebKit 只在内联 style 变化时重绘自定义滚动条，改 class/属性不生效。
- `src/features/*`：各功能模块；对话渲染在 `features/transcript/`（TanStack Virtual 虚拟列表，`nodes/` 节点渲染，`tools/` 工具渲染器）。Shiki 在 Worker 中运行。`@/` 指向 `src/`。

## 约束

- **对数据根严格只读**（ADR-0002）：源文件只能用 `File::open` 读，绝不写入、移动、删除 `~/.claude` 下任何东西。应用自己的状态只放在 `~/Library/Caches/dev.joy.codeck/`（索引、价格表缓存）和 `~/Library/Application Support/dev.joy.codeck/`（设置）。
- **测试 fixture**：`crates/codeck-core/tests/fixtures/<场景>/` 是迷你数据根，由 `examples/sanitize.rs` 从真实 Session 脱敏生成；测试通过 `common::fixture_root(name)` 复制到临时目录使用，不要直接写 fixture 目录。`tests/fixtures_sanity.rs` 钉死了 fixture README 表格里的每个数字，并做敏感信息检查（`CODECK_FIXTURE_DENYLIST` 追加私有词）——改动或新增 fixture 时同步更新 README 表格和该测试，绝不提交未脱敏的真实数据。
- 启动、后台调度、设置这类流程单测覆盖不到，需要 `pnpm tauri dev` 实测。
- 在 macOS 27 上 release 构建依赖根 `Cargo.toml` 中 `profile.release.build-override` 的 `strip = "none"`，不要删。
- Commit message 风格：`feat|fix|docs|test|style: <中文描述>`，常用「——」补充说明原因或做法。
