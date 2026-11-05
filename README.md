# CoDeck

macOS 桌面应用：搜索和阅读 Claude Code 留在本机 `~/.claude` 下的历史 Session，并能 Connect ToH clay 里正在运行的 Session。名字合写了 code、co-deck（与终端共显）和 deck；显示名写作 CoDeck，代码与路径里用小写 `codeck`。

- **找得到**：⌘K 一个搜索框同时查 Project、Session（标题、Project、分支，即时）和对话内容（全文索引，中英文子串匹配、`"短语"`、`-排除词`）；选 Project 可筛选列表或限定搜索范围；命中直接跳到对应消息，包括 Subagent 内部和已回退的分支。在工作区根目录启动、改动横跨多个仓库的 Session 会出现在它改过的每个仓库下。
- **读得舒服**：参考 Codex 的排版，每轮的过程折叠成「已处理 1 分 50 秒」一行、只露出回答，展开后每次工具调用是一行字（Subagent 面板里连续的合成「读取 1 个文件、编辑 2 个文件、运行 1 条命令」一行），Bash 终端输出、Edit diff、代码高亮按需展开；Subagent 在右侧面板查看完整过程；Branch 切换、Fork 折叠、compact 分隔；⌘F 会话内查找、`j`/`k` 按轮次跳转、轮次大纲；Session 列表可收起，竖屏也有宽敞的对话记录；对话列宽度拖动边缘即可调节。
- **实时**：监听文件变化，新 Session 自动出现；正在进行的 Session 实时追随。
- **Cost**：参照 ccusage 的算法按 API 单价估算每个 Session 的花费（含 Subagent，resume / Fork 复制的历史不重复计算），Session 列表可按 Cost 排序，统计页汇总总额与 Session 排行。
- **Connect**：ToH clay 里装了 ccrpc 插件的 Session，在标题栏点「Connect」后可以直接发提示词（可粘贴截图）和斜杠命令（输入 `/` 补全内置、自定义、插件的命令与 skill）、忙时插话（和终端里一样进 Claude Code 的队列）、作答审批 / 提问 / 选择、切换权限模式、模型与思考等级、停止后台任务、回退到 Main Line 上某条提示词之前（悬停提示词点「回退」或输入 `/rewind`（`/checkpoint`），可选恢复代码与对话、只恢复对话或只恢复代码）；「关闭」结束 Session 并关掉它的 Pane；也可以选目录新建 Session，或把已结束的 Session 在新 Pane 里 Resume（`claude --resume`）后接着 Connect，并按目录给 claude 追加参数（如 `--add-dir`）。对话内容仍来自本应用的索引与实时追随，ccrpc 只提供状态与控制（见 [crates/ccrpc](crates/ccrpc)）。
- **统计**：默认看本周期（自定义每周重置时间）的用量，可切到月份或最近 7 / 30 / 90 天，周期和月份可前后翻；输入、输出 token 与缓存命中率，每日 token（全部 / 输入 / 输出，后两者按模型）、按时间范围自动分桶的活跃时段、Session 排行，图表可下钻到 Session 列表。

## 快捷键

| 按键 | 作用 |
|---|---|
| ⌘K | 搜索 Session、Project 和对话内容（⇥ 跳段，Project 上 ⌘↵ 限定范围） |
| ⌘F | 在当前 Session 内查找（Enter / ⇧Enter 上下跳转） |
| `j` / `k` | 跳到下一个 / 上一个你的输入 |
| ⌘↑ / ⌘↓ | 跳到对话顶部 / 底部 |
| ↑ / ↓ | 在 Session 列表中切换（列表获得焦点时） |
| ⌃⌘L | 收起 / 展开 Session 列表 |
| ⌘, | 设置（数据目录、ccrpc、格式兼容性诊断、重建索引） |
| Esc | 关闭搜索、查找或 Subagent 面板；Connect 后选回退点时（没有这些要关）退出选回退点 |
| Enter / ⌘Enter | Connect 后：空闲发送；忙时插话（当前工具调用结束后送达）/ 中断本轮立即发送（同终端里的 ctrl+enter）；等待作答时把焦点移到请求卡片 |
| Esc（焦点在输入区） | Connect 后：忙或等待作答时中止本轮（回退的确认卡片上是取消回退，正在准备回退时不起作用）；卡片里的输入框中只离开输入框 |
| 1 / 2 / 3 | Connect 后：选第一张请求卡片的选项（焦点在输入区内、不在输入框时） |

术语见 [CONTEXT.md](CONTEXT.md)，完整规格见 [docs/spec.md](docs/spec.md)，关键取舍见 [docs/adr/](docs/adr/)。

## 只读保证

应用从不写入、移动或删除数据目录（默认 `~/.claude`）下的任何文件（[ADR-0002](docs/adr/0002-read-only.md)）。Connect 后对 Claude Code 的写入只经 ccrpc（向 Pane 的 PTY 写入、回复 ccrpc 插件的审批请求），应用仍不写数据目录下的任何文件。应用自己的状态只放在：

| 内容 | 位置 |
|---|---|
| 索引（可丢弃，随时重建） | `~/Library/Caches/dev.joy.codeck/index.sqlite` |
| 价格表缓存 | `~/Library/Caches/dev.joy.codeck/pricing.json` |
| 设置（数据目录覆盖） | `~/Library/Application Support/dev.joy.codeck/settings.json` |
| ccrpc 日志（按连接分目录，保留 14 天） | `~/Library/Logs/dev.joy.codeck/ccrpc/` |
| 输入区粘贴的图片 | `~/Library/Caches/dev.joy.codeck/images/` |

应用唯一的联网请求是在后台下载 LiteLLM 的公开价格表（缓存超过一天才重新下载），用来计算 Cost；不发送任何本机数据，下载失败时使用内置快照（[ADR-0005](docs/adr/0005-fetch-litellm-prices.md)）。

索引只镜像数据目录当前内容：源文件被删，对应 Session 也随之消失；升级后索引格式变化会自动重建（[ADR-0003](docs/adr/0003-index-is-disposable-mirror.md)）。索引放在 Caches 中，不进入 Time Machine 备份。

首次启动时先扫描元数据（4.7 GB 数据约 5 秒），列表即可浏览；全文索引在后台建立（约 1 分钟，索引约 1 GB），期间 Session 列表底部显示进度，搜索结果不完整。工具输出不进索引，也不提供搜索（[ADR-0004](docs/adr/0004-tool-output-scanned-not-indexed.md)）。

## 数据目录

按以下顺序确定：设置中的覆盖 → 环境变量 `CLAUDE_CONFIG_DIR` → `~/.claude`。

从 Finder / Dock 启动的应用读不到 shell 里的环境变量，如果使用了 `CLAUDE_CONFIG_DIR`，请在「设置 → 数据」（⌘,）中填写路径。

## 环境要求

- macOS 14+，Apple Silicon
- Rust 1.91+（`rustup`）、Xcode Command Line Tools
- Node 22+ 与 pnpm
- Connect（可选）：ToH（clay 后端）与 ccrpc；ccrpc 依次取 `CODECK_CCRPC`、`~/.local/bin/ccrpc`、PATH 里的 `ccrpc`，插件用 `ccrpc plugin install` 安装（见 [crates/ccrpc](crates/ccrpc)）

## 开发

```sh
pnpm install
pnpm tauri dev          # 启动应用（读取真实数据目录）
pnpm dev:mock           # 只启动前端，使用内置 mock 数据（浏览器打开 http://localhost:1420）
```

Rust 侧的 IPC 类型是唯一来源，修改 `crates/transcript/src/model/*`（组装用的类型）或 `crates/codeck-core/src/model/*`（应用层类型）后重新生成前端绑定：

```sh
pnpm bindings           # 写入 src/ipc/bindings.ts（勿手改）
```

## 测试

```sh
cargo test --workspace                                  # 解析、组装、索引、搜索、统计、诊断（基于脱敏 fixture）
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                # 提交前格式化 Rust 代码
pnpm format                    # 提交前格式化前端代码（Prettier）；pnpm format:check 只检查
scripts/install-git-hooks.sh   # 一次性：装 pre-commit 钩子，Rust 或前端代码没格式化、hooks 规则不过时拦下提交
pnpm typecheck && pnpm lint && pnpm test                # 前端类型检查、hooks 规则检查（ESLint）与单元测试
pnpm exec playwright install webkit && pnpm test:e2e    # E2E（mock 模式，WebKit）
```

- `E2E_PORT=<端口>`：E2E 的 mock 开发服务器端口（默认 1421），并行跑多份 E2E 时各用一个。
- `CODECK_FIXTURE_DENYLIST="词1,词2"`：fixture 脱敏检查额外拦截的私有词（如公司、客户名），不写进仓库。
- 真实数据探针（只读，索引写入临时目录），用于检查性能与兼容性：

```sh
CODECK_REAL_ROOT=~/.claude cargo test -p transcript --release --test assemble -- --ignored --nocapture    # 组装全部 Session
CODECK_BENCH_ROOT=~/.claude cargo test -p codeck-core --release --test search -- --ignored --nocapture    # 建全文索引并测查询耗时
```

内置价格快照（`crates/transcript/src/pricing-snapshot.json`）的刷新方式：

```sh
curl -sSf https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json \
  | cargo run -p transcript --example update_pricing
```

`crates/codeck-core/tests/fixtures/` 是从真实 Session 精简、脱敏得到的结构样本（见其中的 README），由 `crates/codeck-core/examples/sanitize.rs` 生成；`crates/transcript/tests/` 里解析层的测试也读这份 fixture。

## 构建

```sh
pnpm tauri build --bundles app --target aarch64-apple-darwin
```

产物在 `target/aarch64-apple-darwin/release/bundle/macos/CoDeck.app`，使用 ad-hoc 签名、未公证；首次打开如被 Gatekeeper 拦截，右键选择「打开」。

安装到「应用程序」（先退出正在运行的旧版本）：

```sh
ditto "target/aarch64-apple-darwin/release/bundle/macos/CoDeck.app" "/Applications/CoDeck.app"
```

在 macOS 27 上，`tauri build` 设置的 `MACOSX_DEPLOYMENT_TARGET=14.0` 会让 cargo strip 后的 proc-macro 动态库无法加载（`mis-aligned LINKEDIT string pool`），因此 `Cargo.toml` 中对 `profile.release.build-override` 关闭了 strip，不影响最终应用。

## 结构

```
crates/transcript/ 解析库：容错解析、对话组装、组装用的显示模型、进程文件、价格（ccrpc 也用它）
crates/codeck-core/   索引与查询：扫描、SQLite 索引（FTS5 trigram）、搜索、统计、诊断；应用层的 IPC 类型
crates/ccrpc/     ccrpc：以 JSONL 驱动 ToH clay 里的 Claude Code（见其 README）
src-tauri/        Tauri 外壳：命令与事件、后台 worker、FSEvents 监听、Live 轮询、设置
src/              React 前端：features/* 各功能模块，ipc/ 为 IPC 层（real / mock 双实现）
e2e/              Playwright 用例
```

**CoDeck、ccrpc 与 ToH 的分工**：ToH 在本机用 Holder 持有 clay Pane 的 PTY，Claude Code 跑在里面，Mac / iPhone / web 的 Client 都能接手同一个 Pane。ccrpc 把一个 Pane 里的交互式 Claude Code 变成 JSONL 接口：经 Holder 收屏幕、写按键，经 ccrpc 插件的 hooks 得知审批、提问与轮次边界并代答审批。CoDeck 每 Connect 一个 Session 起一个 `ccrpc attach` 子进程（Session 同时显示在终端里，两边都能输入）（一个 Session 同时只能连一个 ccrpc），Disconnect 只结束这个 ccrpc，Session 照常运行。对话内容始终来自 CoDeck 自己的索引与实时追随，不经 ccrpc（[ADR-0007](docs/adr/0007-monorepo-parsing-library-and-drive-via-ccrpc.md)）。

## 许可证

Copyright (C) 2026 XGFan

本项目以 [GNU Affero General Public License v3.0 或更高版本](LICENSE)（AGPL-3.0-or-later）发布。你可以自由使用、修改和分发；分发修改版，或以网络服务形式向他人提供修改版时，须以同一许可证公开对应源码。
