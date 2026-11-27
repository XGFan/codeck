# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

CoDeck：macOS 桌面应用（Tauri 2 + React 19），搜索和阅读 `~/.claude` 下 Claude Code 的历史 Session，并经 ccrpc Connect ToH clay 里运行中的 Session（ADR-0007）；原生 iPhone App 经局域网浏览与 Connect 一台 Host：打开了「局域网访问」的桌面应用（ADR-0012），或 headless daemon（ADR-0011）；局域网上走 TLS、扫码配对（ADR-0017）。显示名写 CoDeck，代码、包名、路径与环境变量里写小写 `codeck` / `CODECK_`。

## 仓库布局（monorepo）

| 目录 | 内容 | 细节 |
|---|---|---|
| `crates/transcript` | 解析库：读 Claude Code 的数据（transcript、进程文件、价格），组装成显示模型；无 SQLite、无 tauri | [crates/transcript/CLAUDE.md](crates/transcript/CLAUDE.md) |
| `crates/codeck-core` | 索引、搜索、统计、诊断（SQLite + FTS5）；纯 Rust | [crates/codeck-core/CLAUDE.md](crates/codeck-core/CLAUDE.md) |
| `crates/codeck-host` | Host 的运行时（worker、监听、设置、ccrpc、Connect 状态、局域网服务）与 headless daemon `codeck-host` | [crates/codeck-host/CLAUDE.md](crates/codeck-host/CLAUDE.md) |
| `desktop` | 桌面应用的 Tauri 外壳（原 `src-tauri`） | [desktop/CLAUDE.md](desktop/CLAUDE.md) |
| `web` | React 前端：源码 `web/src`（mock 后端在 `web/src/mocks`），三个入口 html，Vite / TypeScript / ESLint / Playwright 的配置，E2E 在 `web/e2e/`（mock）与 `web/e2e/host/`（真实后端） | [web/CLAUDE.md](web/CLAUDE.md) |
| `ios` | iPhone App（SwiftUI，xcodegen） | [ios/CLAUDE.md](ios/CLAUDE.md) |
| `crates/ccrpc` | ccrpc：以 JSONL 驱动 ToH clay 里的 Claude Code | [crates/ccrpc/CLAUDE.md](crates/ccrpc/CLAUDE.md) |
| `crates/codeck-cli` | 命令行 `codeck`：给 AI 查询 Session 与新建 Session（ADR-0013） | [crates/codeck-cli/CLAUDE.md](crates/codeck-cli/CLAUDE.md) |
| `crates/codeck-term` | 终端窗口的 tauri 插件：认不出的界面直接在 Pane 的终端里处理（过渡用，ADR-0015） | 本文件「终端窗口」 |

各目录的 CLAUDE.md 只在读到那个目录下的文件时才加载，本文件只放全局的东西：命令、数据流、跨模块的规则与约束。改哪一块，先看哪一份。

一个 Cargo workspace（`cargo test --workspace` 覆盖全部 crate）；前端在 `web/`，`package.json` 与锁文件留在根目录，`pnpm` 的命令都在根目录跑。原 cc-gui 已并入 viewer 的 Connect 功能（ADR-0007）。Connect E2E 的 wire fixture 在 `web/src/mocks/data/connect/`，`tools.jsonl` 用 `scripts/record-connect-fixtures.sh` 从 ccrpc 的 mock 端到端测试重录。

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
pnpm format                    # 前端格式化（Prettier，printWidth 120；范围见 .prettierignore：不含 crates、desktop、Markdown、bindings.ts）；提交前必须是格式化过的
scripts/install-git-hooks.sh   # 装 pre-commit 钩子：暂存了没按 rustfmt / Prettier 排版、或违反 hooks 规则的文件就拦下；与 pre-push 钩子
scripts/check.sh               # 推送前的检查（代替 CI，pre-push 钩子跑的就是它；--all 不看改动全跑）：改了 Rust 跑 fmt / clippy / cargo test，改了 ccrpc 加 mock e2e，改了前端跑 Prettier / typecheck / lint / vitest / E2E，改了 daemon 或它提供的对话页会读到的文件（codeck-host / codeck-core / transcript、前端）加真实后端 E2E，共几分钟；每组按自己会读到的文件记通过（只改前端的提交不会让 Rust 与 ccrpc mock e2e 重跑；`--keys` 看各组的键），同样的内容通过过就跳过，所以提交后先跑它，git push 就不用再等。git push 没有输出不是卡住，是钩子在跑

pnpm typecheck && pnpm lint && pnpm test           # tsc + ESLint（react-hooks 两条规则，加导入方向：ipc / lib / components / state / queries 不引用 features，mock 只经 ipc/index.ts 接入，features/connect/state 不引用对话记录、终端窗口与 Connect 的组件）+ vitest（src/**/*.test.ts）
pnpm test web/src/lib/diff.test.ts                 # 单个前端单测
pnpm exec playwright install webkit                # 首次跑 E2E 前
pnpm test:e2e                                      # Playwright，WebKit，mock 模式：先构建到 dist-e2e/<端口>/ 再起静态服务（在 web/ 下；端口上已有服务就用它）；同一文件里的用例也并行
pnpm test:e2e web/e2e/search.spec.ts -g "<title>"  # 单个 E2E
E2E_PORT=1431 pnpm test:e2e                        # 并行跑多份时各用一个端口（默认 1421）

pnpm bindings                  # 改了 IPC 类型/命令后重新生成 web/src/ipc/bindings.ts
node scripts/demo/build.ts     # 重新生成演示数据根 demo/root（故事在 demo/story，见 demo/README.md）
curl -sSf https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json \
  | cargo run -p transcript --example update_pricing   # 刷新内置价格快照
pnpm tauri build --bundles app --target aarch64-apple-darwin

cargo run -p codeck-cli -- sessions --limit 5   # 命令行（只读打开桌面的索引）；用法见 crates/codeck-cli/SKILL.md
pnpm build:web                 # iPhone 用的只读 Transcript 页 → web/dist-web/（vite --mode host）
cargo run -p codeck-host -- --listen 127.0.0.1 --state-dir /tmp/h --web-dir web/dist-web   # 本机试 daemon（别用桌面的状态目录）
pnpm test:e2e:host             # 真实后端 E2E（web/e2e/host）：起 codeck-host 读 fixture 副本
cd ios && xcodegen generate && xcodebuild -project CoDeck.xcodeproj -scheme CoDeck -destination 'platform=iOS Simulator,name=iPhone 15 Pro Max' test
```

真实数据探针（`#[ignore]`，只读，索引写临时目录）：`CODECK_REAL_ROOT=~/.claude cargo test -p transcript --release --test assemble -- --ignored --nocapture`、`CODECK_BENCH_ROOT=~/.claude cargo test -p codeck-core --release --test search -- --ignored --nocapture`、`CODECK_REAL_ROOT=~/.claude cargo test -p codeck-core --release --test diag -- --ignored --nocapture`（格式兼容性诊断，含认不出的消息）。

## 设计稿

界面设计统一放在 Claude Design 画布「CoDeck 设计」：https://claude.ai/artifact/M5J8o7P5DFasg3dYsi2dxs ，不另开画布，只放可编辑的原型图（不放截图）。

- 按应用区域分页：1 · 窗口总览、2 · 对话记录、3 · 输入区与 Connect、4 · 搜索（⌘K）、5 · 统计。区域页只放当前的原型，右侧一列便签写规则；不按时间或「方案 / 已实现」分页。
- 新设计：另建「方案 · <主题>」页出 2–3 个可编辑画板，选定后再实现；实现后把采用的画板挪进对应区域页（标题、说明里去掉「方案」「已实现」字样），删掉方案页。
- 之后改了界面，把区域页里对应的原型图与便签一起改到和实现一致。

## 架构

数据单向流动：`crates/transcript`（解析）→ `crates/codeck-core`（索引、搜索、统计；纯 Rust，无 tauri 依赖）→ `crates/codeck-host`（运行时：worker、监听、设置、ccrpc、Connect 状态；无 tauri 依赖）→ 外壳：`desktop`（桌面，进程内调用）或 daemon（`crates/codeck-host/src/bin/codeck-host.rs`，HTTP + WebSocket）→ `src`（React）/ `ios`（SwiftUI）。ccrpc 也依赖 `crates/transcript`，不依赖 codeck-core。

### 跨模块的规则

- 管线：`scan`（遍历数据根、按 realpath/inode 去重）→ `raw`（宽容的 serde 结构，字段全可选）→ `parse`（按行解析，坏行跳过并计数）→ `assemble/*`（把事件日志组装成节点：parentUuid 树、Branch/Main Line、compact 接续、Fork、Subagent/Workflow 关联、工具摘要、⌘F 查找、搜索跳转）→ `index/*`（SQLite + FTS5 trigram）→ `search` / `stats` / `diag`。`known.rs` 列出已知的条目类型/内容块/工具名，未知的进诊断而不是丢弃。
- IPC 类型分两处，都由 specta 导出：`crates/transcript/src/model/*` 放组装用的类型（Transcript、节点、Branch、⌘F / 跳转、Live 状态等），`crates/codeck-core/src/model/*` 放应用层类型（Project、Session 列表与详情、搜索、统计、诊断、应用状态、事件、`AppError`），并重新导出前者，所以 codeck-host 与 desktop 一律写 `codeck_core::model::…`。改这两处或 `crates/codeck-host/src/methods.rs` 命令表的签名后必须 `pnpm bindings`；`web/src/ipc/bindings.ts` 是生成文件，勿手改。
- **命令表只定义一次**：`methods.rs` 的 `for_each_method!` 列出每个命令（`remote` 或 `local`、参数、返回类型、文档注释），codeck-host 由它生成 `dispatch`，desktop 的 `commands.rs` 由它生成 tauri 命令（名字、参数、顺序决定 bindings）。新增命令：在 `Host` 上写同名 async 方法（`commands.rs` / `ccrpc/mod.rs`，阻塞工作经 `tokio::task::spawn_blocking`，错误转 `AppError`，`message` 是面向用户的中文文案），再加进表里；`local` 只给桌面（设置、数据根、Finder 等），daemon 只暴露 `remote`。
- **E2E 全部跑在 mock 上**（tauri-driver 不支持 macOS）：新增/修改 IPC 能力时要同步 `web/src/mocks/` 的实现和 mock 数据。

### 终端窗口（过渡用，ADR-0015）

- 认不出的界面卡片上的「在终端窗口中处理」与「···」菜单的「显示 Terminal」打开 `term-<Holder ID>` 窗口（`web/terminal.html`，前端 `features/terminal`，入口 `web/src/terminal-main.tsx`），xterm.js 经 `crates/codeck-term`（tauri 插件，命令 `plugin:codeck-term|…`，权限在 `desktop/capabilities/terminal.json` 与主窗口的 `codeck-term:main-window`）直连 Holder 的 unix socket；带 `size_events` 附着，从不发 `Resize`。不进 `Api` 与 bindings，mock 模式下是 `window.open` 的弹窗回放固定屏幕。主窗口按 Connect 状态给它推状态、在 Pane 没了（不在 `ccrpc ls` 里、也没有连接）时关掉它（`features/terminal/windows.ts`）。
- 整块可删：crate、`desktop` 里的 `.plugin(codeck_term::init())`、两处 capability、`terminal.html` 与 vite 的 input、`features/terminal`、`App.tsx` 的 `useTerminalWindows()`、ScreenCard 的按钮与 `ConnectMenuItems` 的一项。

### Code Mode（实验功能，格式可能变、也可能被删）

- **读历史的一半留着**（旧 Session 里一直有这些记录）。只有 `assemble/code_mode.rs` 知道原始格式：别处不读 `is_virtual`、不比较 `REPL` / `Poll` 工具名，只调它的函数（模块注释列了每个入口给谁用），格式变了改这一个文件。下游只认模型的 `code` / `viaCode`：前端的规则在 `features/transcript/codeMode.ts`（代码本身不算一次调用、只提问的代码并进提问卡片、出错与「执行 N 段代码」的计数；`grouping.ts` / `fold.ts` 不直接读这两个字段），渲染器是 `tools/code.tsx`，另有 `nodes/Ask.tsx` 的代码折叠、⌘F（`find.rs`）与 CLI（`show.rs`）各一处。认不出的不丢：找不到归属的虚拟消息（如代码工具不叫 `REPL`）按普通工具调用显示；`Poll` 只在结果全部认得出时隐藏（`CodeRuns::quiet_polls`，认得的事件种类是 `QUIET_KINDS` 加 `repl-eval`），否则带原始结果显示成普通工具调用；不认识的事件种类经 `unrecognized_events` 进诊断的「认不出的消息」。
- **实时的一半整块可删**（代码运行期间的调用提前显示出来；Claude Code 没有 Code Mode 了就用不上）：ccrpc 的 `session/code_calls.rs` 与协议里的 `code_call` / `code_call_end` / `ready.code_calls`、请求上的 `from_code`（`requests/Requests.tsx` 的 `ownCard`）；Host `connect/state.rs` 的 `code_calls`；前端 `state/conn.ts` 的 `codeCalls`、`web/src/ipc/protocol/ccrpc.ts` 的对应类型、`state/codeCalls.ts` 与 `rewind/RewindViews.tsx` 里对 `withCodeCalls` 的调用、`state/native.ts` 的 `codeCalls`；iOS 的 `ConnState.swift` 与 `TranscriptWebView.swift`；两份 golden（`conn_state_golden.rs`、`ios/Tests/Fixtures/conn-state.json`）。
- 测试各自成文件，改格式时跑这几条：`cargo test --workspace code_mode`（`crates/transcript/tests/code_mode.rs`，别的 crate 里的用例名都以 `code_mode_` 开头）与 `cargo test -p codeck-host -p ccrpc code_call`（实时的一半）、`pnpm test web/src/features/transcript/codeMode.test.ts web/src/features/connect/state/codeCalls.test.ts`、`pnpm test:e2e web/e2e/code-mode.spec.ts`。fixture 是 `tests/fixtures/code_mode/`，mock 是 `web/src/mocks/data/codeMode.ts` 与 `connect/code-mode.jsonl`。

## 约束

- **对数据根严格只读**（ADR-0002）：源文件只能只读打开（`File::open`，或只读的 `OpenOptions`），绝不写入、移动、删除 `~/.claude` 下任何东西。应用自己的状态只放在 `~/Library/Caches/dev.joy.codeck/`（索引、价格表缓存）和 `~/Library/Application Support/dev.joy.codeck/`（设置）。
- **测试 fixture**：`crates/codeck-core/tests/fixtures/<场景>/` 是迷你数据根，由 `examples/sanitize.rs` 从真实 Session 脱敏生成；测试通过 `common::fixture_root(name)` 复制到临时目录使用，不要直接写 fixture 目录。`tests/fixtures_sanity.rs` 钉死了 fixture README 表格里的每个数字，并做敏感信息检查（`CODECK_FIXTURE_DENYLIST` 追加私有词）——改动或新增 fixture 时同步更新 README 表格和该测试，绝不提交未脱敏的真实数据。
- 启动、后台调度、设置这类流程单测覆盖不到，需要 `pnpm tauri dev` 实测。
- 在 macOS 27 上 release 构建依赖根 `Cargo.toml` 中 `profile.release.build-override` 的 `strip = "none"`，不要删。
- Commit message 风格：`feat|fix|docs|test|style: <中文描述>`，常用「——」补充说明原因或做法。
