# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

CoDeck：macOS 桌面应用（Tauri 2 + React 19），搜索和阅读 `~/.claude` 下 Claude Code 的历史 Session，并经 ccrpc Connect ToH clay 里运行中的 Session（ADR-0007）。显示名写 CoDeck，代码、包名、路径与环境变量里写小写 `codeck` / `CODECK_`。

## 仓库布局（monorepo）

| 目录 | 内容 | 说明 |
|---|---|---|
| `crates/transcript` | 解析库：读 Claude Code 的数据（transcript、进程文件、价格），组装成显示模型；无 SQLite、无 tauri | 本文件「架构」 |
| `crates/codeck-core`、`src-tauri`、`src` | CoDeck 本体 | 本文件 |
| `crates/ccrpc` | ccrpc：以 JSONL 驱动 ToH clay 里的 Claude Code | 见 [crates/ccrpc/CLAUDE.md](crates/ccrpc/CLAUDE.md) |

一个 Cargo workspace（`cargo test --workspace` 覆盖全部 crate）；前端只有根目录的 viewer。原 cc-gui 已并入 viewer 的 Connect 功能（ADR-0007）。Connect E2E 的 wire fixture 在 `src/mocks/data/connect/`，`tools.jsonl` 用 `scripts/record-connect-fixtures.sh` 从 ccrpc 的 mock 端到端测试重录。

- 术语以 [CONTEXT.md](CONTEXT.md) 为准（Session / Project / Live Session / Subagent Run / Workflow Run / Branch / Main Line / Fork / Teammate Message / Connect / Connected Session / Disconnect / Resume），代码、注释、文案都用这些词，避开其中列出的 _Avoid_ 词。
- 行为规格见 [docs/spec.md](docs/spec.md)（计数口径、用户输入判定、Main Line 规则等都在这里），关键取舍见 [docs/adr/](docs/adr/)。改动对外行为时同步更新 spec / README / CONTEXT。

## 常用命令

```sh
pnpm install
pnpm tauri dev                 # 启动真实应用（读取真实数据目录）
pnpm dev:mock                  # 只起前端 + 内置 mock 数据，http://localhost:1420

cargo test --workspace
cargo test -p transcript --test parse                  # 单个集成测试文件（tests/*.rs）
cargo test -p transcript --test branch <name>          # 单个用例
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                # Rust 格式化（默认 rustfmt 配置）；提交前必须是格式化过的
pnpm format                    # 前端格式化（Prettier，printWidth 120；范围见 .prettierignore：不含 crates、src-tauri、Markdown、bindings.ts）；提交前必须是格式化过的
scripts/install-git-hooks.sh   # 装 pre-commit 钩子：暂存了没按 rustfmt / Prettier 排版、或违反 hooks 规则的文件就拦下

pnpm typecheck && pnpm lint && pnpm test           # tsc + ESLint（只开 react-hooks 两条规则）+ vitest（src/**/*.test.ts）
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

真实数据探针（`#[ignore]`，只读，索引写临时目录）：`CODECK_REAL_ROOT=~/.claude cargo test -p transcript --release --test assemble -- --ignored --nocapture`、`CODECK_BENCH_ROOT=~/.claude cargo test -p codeck-core --release --test search -- --ignored --nocapture`、`CODECK_REAL_ROOT=~/.claude cargo test -p codeck-core --release --test diag -- --ignored --nocapture`（格式兼容性诊断，含认不出的消息）。

## 设计稿

界面设计统一放在 Claude Design 画布「CoDeck 设计」：https://claude.ai/artifact/M5J8o7P5DFasg3dYsi2dxs ，不另开画布，只放可编辑的原型图（不放截图）。

- 按应用区域分页：1 · 窗口总览、2 · 对话记录、3 · 输入区与 Connect、4 · 搜索（⌘K）、5 · 统计。区域页只放当前的原型，右侧一列便签写规则；不按时间或「方案 / 已实现」分页。
- 新设计：另建「方案 · <主题>」页出 2–3 个可编辑画板，选定后再实现；实现后把采用的画板挪进对应区域页（标题、说明里去掉「方案」「已实现」字样），删掉方案页。
- 之后改了界面，把区域页里对应的原型图与便签一起改到和实现一致。

## 架构

三层，数据单向流动：`crates/transcript`（解析）→ `crates/codeck-core`（索引、搜索、统计；纯 Rust，无 tauri 依赖）→ `src-tauri`（外壳）→ `src`（React）。ccrpc 也依赖 `crates/transcript`，不依赖 codeck-core。

### transcript 与 codeck-core

- `crates/transcript` 放解析层：`raw`、`parse`、`known`、`assemble/*`、`model/*`（组装用的类型）、`live`（进程文件）、`layout`（Session 目录里 agent 与 sidecar 文件的位置，codeck-core 的扫描与 ccrpc 共用）、`pricing`。codeck-core 直接写 `transcript::…`。解析层的集成测试（parse / merge / branch / assemble）在 `crates/transcript/tests/`，fixture 用 codeck-core 的，经 `#[path]` 引用 `crates/codeck-core/tests/common/fixtures.rs`（该文件不能依赖 codeck-core）。解析层的错误是 `TranscriptError`，codeck-core 按变体映射到 `CoreError`。

- `engine/` 的 `Engine` 是 src-tauri 调用的**唯一入口**，全部是阻塞调用（门面 API 在 `engine/mod.rs`）。它负责：索引同步（`engine/sync/`：stat 遍历（FS 批次的路径都落在某些 Session 里时只 stat 这些 Session，启动、Rescan 与其他情况全扫）→ 与 `files` 表比对（`plan.rs`）→ rayon 并行轻解析（`compute.rs`）→ 按批事务写入（`write.rs`）→ 重算受影响的 Session；追加只读 `[parsed_offset, EOF)`，文件变小/被替换则整 Session 重解析）；以及显示用的小 LRU（`engine/session_cache.rs`，按 source/sidecar 的 path+size+mtime 哈希校验）。
- 管线：`scan`（遍历数据根、按 realpath/inode 去重）→ `raw`（宽容的 serde 结构，字段全可选）→ `parse`（按行解析，坏行跳过并计数）→ `assemble/*`（把事件日志组装成节点：parentUuid 树、Branch/Main Line、compact 接续、Fork、Subagent/Workflow 关联、工具摘要、⌘F 查找、搜索跳转）→ `index/*`（SQLite + FTS5 trigram）→ `search` / `stats` / `diag`。`known.rs` 列出已知的条目类型/内容块/工具名，未知的进诊断而不是丢弃。
- IPC 类型分两处，都由 specta 导出：`crates/transcript/src/model/*` 放组装用的类型（Transcript、节点、Branch、⌘F / 跳转、Live 状态等），`crates/codeck-core/src/model/*` 放应用层类型（Project、Session 列表与详情、搜索、统计、诊断、应用状态、事件、`AppError`），并重新导出前者，所以 src-tauri 一律写 `codeck_core::model::…`。改这两处或 `src-tauri/src/commands.rs` 的签名后必须 `pnpm bindings`；`src/ipc/bindings.ts` 是生成文件，勿手改。
- 索引是可丢弃的镜像（ADR-0003）：不写迁移，schema 变化就把 `index/schema.rs` 的 `SCHEMA_VERSION` +1，启动时自动删库重建。
- 工具输出不进 FTS，也不提供搜索（ADR-0004）；⌘F 会话内查找仍覆盖工具输出（`assemble/find.rs`）。
- Cost：`pricing.rs` 按 ccusage 的算法给单条消息定价，通过 SQLite 函数 `cv_cost` 写入 `messages.cost`。`writer::rebill` 实现去重：同一 `message.id` 出现在多个 Session 时只把最早 Session 那份标为 `billed`，Session 的 token / 金额列和统计只累加 billed 的消息。每次同步结束时，对变化的 Session 及与它们共享消息的 Session 重算（`engine/sync/write.rs` 的 `rebill_changed`）。价格表在 `Engine` 里，联网下载在 src-tauri（`prices.rs`，ADR-0005）。

### src-tauri

- `commands.rs`：每个命令都是 async，通过 `with_engine` 把 Engine 调用丢到 blocking pool，错误转成 `AppError`（`message` 是面向用户的中文文案）。新增命令要同时登记到 `lib.rs` 的 `specta_builder()`。
- `worker.rs`：单个后台 worker 通过 channel 消费 `Job`（FullScan / Fs 批次（排队中的合并成一个）/ TextBacklog / Rebuild / SetRoot / SwapRoot（切换数据根：在两个任务之间换 Engine，命令等它回复）/ Rescan / RefreshPrices / ApplyPrices）。Live 轮询不排进 worker：计时线程每 3 秒直接读进程文件，免得排在索引后面、又打断全文积压。全文索引积压 `TextBacklog` 在空闲时运行，任何其他任务入队都会让它在当前事务提交后让位——改调度逻辑时注意别造成互相重新入队的活锁（曾出现过）。
- `prices.rs`：下载 LiteLLM 价格表；worker 的 `RefreshPrices` 在独立线程里下载，结果以 `ApplyPrices` 交回 worker 写库，价格有变化时发 `PricesChanged` 事件。
- `watcher.rs`（FSEvents，debounce 后发 `Fs` 任务）、`state.rs`（持有可整体替换的 Engine，切换数据根时 swap）、`settings.rs`（数据根覆盖，存于 Application Support）、`events.rs`（IndexStatus / SessionsChanged / LiveChanged / PricesChanged，以及 ccrpc 的 CcrpcLine / CcrpcExit / CcrpcLs / CcrpcLsEnded 事件）。
- `ccrpc/`：Connect（ADR-0007）。每个连接一个 `ccrpc attach|new` 子进程，stdout 逐行原样转发为 `CcrpcLine` 事件（协议解析与状态都在前端 `features/connect`），三层日志在 `~/Library/Logs/dev.joy.codeck/ccrpc/`。状态在第一个 ccrpc 命令时才初始化（要跑一次登录 shell 取环境），不要挪到启动流程里。可 Connect 的 Session 列表由常驻的 `ccrpc ls --json --watch` 推来（`relay.rs` 的 `watch_sessions`，一次只有一个）。对话内容不走 ccrpc，仍由索引与 Live 跟随提供。

### src（前端）

- `src/ipc/`：`Api` 接口（`api.ts`）有两套实现——`real.ts` 包装生成的 bindings，`mock.ts` 用 `src/mocks/data/*` 的内存数据。`VITE_IPC=mock`（`.env.mock`，即 `pnpm dev:mock`）切到 mock。**E2E 全部跑在 mock 上**（tauri-driver 不支持 macOS），所以新增/修改 IPC 能力时要同步 `mock.ts` 和 mock 数据。
- `src/queries/`：TanStack Query hooks 与 `queryKeys`；`events.ts` 订阅后端事件并做节流失效（实时追随、统计刷新）。
- `src/state/ui.ts`：zustand 的 UI 状态；按 Session 的状态（branchChoices、panelStack、⌘F、展开状态等）在切换 Session 时重置；展开状态的键统一由 `features/transcript/keys.ts` 生成；栏的收起状态（`collapsed`）是全局的，存 localStorage。
- 配色：`lib/theme.ts` 按设置（自动 / 浅色 / 暗色，存 localStorage）在 `<html>` 上写 `data-theme`，`index.css` 的暗色变量、Tailwind 的 `dark:`（`@custom-variant`）、图表与 Shiki 都按它，不直接用 `prefers-color-scheme`；`public/theme-init.js` 在首次绘制前套用；窗口本身经 Tauri 的 `setTheme` 跟着切换。
- 滚动条：`index.css` 自定义 `::-webkit-scrollbar`，`lib/scrollbars.ts` 在滚动时给元素写内联 `--scroll-thumb` 让滑块显示。WebKit 只在内联 style 变化时重绘自定义滚动条，改 class/属性不生效。
- `src/features/*`：各功能模块；对话渲染在 `features/transcript/`（TanStack Virtual 虚拟列表，`nodes/` 节点渲染，`tools/` 工具渲染器，列表的贴底跟随、滚动请求等在 `listHooks.ts`）。Connect 的状态在 `features/connect/store.ts`，ccrpc 连接的命令与事件分发在 `client.ts`（可注入 api 与计时器，单测见 `store.test.ts`），`ccrpc ls --watch` 的启停在 `lsWatch.ts`，草稿持久化在 `drafts.ts`。Shiki 在 Worker 中运行（它用到的 shiki 模块列在 `vite.config.ts` 的 `optimizeDeps.include`，否则 Vite 运行中预构建会整页刷新）。`@/` 指向 `src/`。E2E 的公共 helper 在 `e2e/helpers.ts`。

## 约束

- **对数据根严格只读**（ADR-0002）：源文件只能只读打开（`File::open`，或只读的 `OpenOptions`），绝不写入、移动、删除 `~/.claude` 下任何东西。应用自己的状态只放在 `~/Library/Caches/dev.joy.codeck/`（索引、价格表缓存）和 `~/Library/Application Support/dev.joy.codeck/`（设置）。
- **测试 fixture**：`crates/codeck-core/tests/fixtures/<场景>/` 是迷你数据根，由 `examples/sanitize.rs` 从真实 Session 脱敏生成；测试通过 `common::fixture_root(name)` 复制到临时目录使用，不要直接写 fixture 目录。`tests/fixtures_sanity.rs` 钉死了 fixture README 表格里的每个数字，并做敏感信息检查（`CODECK_FIXTURE_DENYLIST` 追加私有词）——改动或新增 fixture 时同步更新 README 表格和该测试，绝不提交未脱敏的真实数据。
- 启动、后台调度、设置这类流程单测覆盖不到，需要 `pnpm tauri dev` 实测。
- 在 macOS 27 上 release 构建依赖根 `Cargo.toml` 中 `profile.release.build-override` 的 `strip = "none"`，不要删。
- Commit message 风格：`feat|fix|docs|test|style: <中文描述>`，常用「——」补充说明原因或做法。
