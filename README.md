# Claude Viewer

只读的 macOS 桌面应用，用来搜索和阅读 Claude Code 留在本机 `~/.claude` 下的历史 Session。

- **找得到**：⌘K 一个搜索框同时查 Project、Session（标题、Project、分支，即时）和对话内容（全文索引，中英文子串匹配、`"短语"`、`-排除词`）；选 Project 可筛选列表或限定搜索范围；命中直接跳到对应消息，包括 Subagent 内部和已回退的分支。在工作区根目录启动、改动横跨多个仓库的 Session 会出现在它改过的每个仓库下。
- **读得舒服**：工具调用折叠成摘要，Bash 终端输出、Edit diff、代码高亮按需展开；Subagent 在右侧面板查看完整过程；Branch 切换、Fork 折叠、compact 分隔；⌘F 会话内查找、`j`/`k` 按轮次跳转、轮次大纲；Session 列表可收起，竖屏也有宽敞的对话区。
- **实时**：监听文件变化，新 Session 自动出现；正在进行的 Session 实时追随。
- **API 金额**：参照 ccusage 的算法按 API 价格估算每个 Session 的花费（含 Subagent，resume / Fork 复制的历史不重复计算），Session 列表可按金额排序，统计页汇总总额与 Session 排行。
- **驾驶**：ToH clay 里装了 ccrpc 插件的 Session，在标题栏点「驾驶」后可以直接发提示词（可粘贴截图）、忙时排队或插话、作答审批 / 提问 / 选择、切换权限模式、停止后台任务；也可以选目录新建 Session。对话内容仍来自本应用的索引与实时追随，ccrpc 只提供状态与控制（见 [crates/ccrpc](crates/ccrpc)）。
- **统计**：可按自定义每周重置时间统计本周期用量；输入、输出 token 与缓存命中率，每日 token（全部 / 输入 / 输出，后两者按模型）、按时间范围自动分桶的活跃时段、Session 排行，图表可下钻到 Session 列表。

## 快捷键

| 按键 | 作用 |
|---|---|
| ⌘K | 搜索 Session、Project 和对话内容（⇥ 跳段，Project 上 ⌘↵ 限定范围） |
| ⌘F | 在当前 Session 内查找（Enter / ⇧Enter 上下跳转） |
| `j` / `k` | 跳到下一个 / 上一个你的输入 |
| ⌘↑ / ⌘↓ | 跳到对话顶部 / 底部 |
| ↑ / ↓ | 在 Session 列表中切换（列表获得焦点时） |
| ⌃⌘L | 收起 / 展开 Session 列表 |
| ⌘, | 设置（数据目录、驾驶、格式兼容性诊断、重建索引） |
| Esc | 关闭搜索、查找或 Subagent 面板 |
| Enter / ⌘Enter | 驾驶时：空闲发送；忙时排到本轮之后 / 立即插话；等待作答时把焦点移到请求卡片 |
| Esc（焦点在驾驶区） | 驾驶时：忙或等待作答时中止本轮；卡片里的输入框中只离开输入框 |
| 1 / 2 / 3 | 驾驶时：选第一张请求卡片的选项（焦点在驾驶区内、不在输入框时） |

术语见 [CONTEXT.md](CONTEXT.md)，完整规格见 [docs/spec.md](docs/spec.md)，关键取舍见 [docs/adr/](docs/adr/)。

## 只读保证

应用从不写入、移动或删除数据目录（默认 `~/.claude`）下的任何文件（[ADR-0002](docs/adr/0002-read-only.md)）。驾驶时对 Claude Code 的写入只经 ccrpc（向 Pane 的 PTY 写入、回复 ccrpc 插件的审批请求），应用仍不写数据目录下的任何文件。应用自己的状态只放在：

| 内容 | 位置 |
|---|---|
| 索引（可丢弃，随时重建） | `~/Library/Caches/dev.joy.claude-viewer/index.sqlite` |
| 价格表缓存 | `~/Library/Caches/dev.joy.claude-viewer/pricing.json` |
| 设置（数据目录覆盖） | `~/Library/Application Support/dev.joy.claude-viewer/settings.json` |
| 驾驶日志（按连接分目录，保留 14 天） | `~/Library/Logs/dev.joy.claude-viewer/drive/` |
| 驾驶时粘贴的图片 | `~/Library/Caches/dev.joy.claude-viewer/images/` |

应用唯一的联网请求是在后台下载 LiteLLM 的公开价格表（缓存超过一天才重新下载），用来计算 API 金额；不发送任何本机数据，下载失败时使用内置快照（[ADR-0005](docs/adr/0005-fetch-litellm-prices.md)）。

索引只镜像数据目录当前内容：源文件被删，对应 Session 也随之消失；升级后索引格式变化会自动重建（[ADR-0003](docs/adr/0003-index-is-disposable-mirror.md)）。索引放在 Caches 中，不进入 Time Machine 备份。

首次启动时先扫描元数据（3 GB 数据约 2–3 秒），列表即可浏览；全文索引在后台建立（约 2–3 分钟，索引约 760 MB），期间状态栏显示进度，搜索结果不完整。工具输出不进索引，也不提供搜索（[ADR-0004](docs/adr/0004-tool-output-scanned-not-indexed.md)）。

## 数据目录

按以下顺序确定：设置中的覆盖 → 环境变量 `CLAUDE_CONFIG_DIR` → `~/.claude`。

从 Finder / Dock 启动的应用读不到 shell 里的环境变量，如果使用了 `CLAUDE_CONFIG_DIR`，请在「设置 → 数据」（⌘,）中填写路径。

## 环境要求

- macOS 14+，Apple Silicon
- Rust 1.90+（`rustup`）、Xcode Command Line Tools
- Node 22+ 与 pnpm

## 开发

```sh
pnpm install
pnpm tauri dev          # 启动应用（读取真实数据目录）
pnpm dev:mock           # 只启动前端，使用内置 mock 数据（浏览器打开 http://localhost:1420）
```

Rust 侧的 IPC 类型是唯一来源，修改 `crates/transcript/src/model/*` 后重新生成前端绑定：

```sh
pnpm bindings           # 写入 src/ipc/bindings.ts（勿手改）
```

## 测试

```sh
cargo test --workspace                                  # 解析、组装、索引、搜索、统计、诊断（基于脱敏 fixture）
cargo clippy --workspace --all-targets -- -D warnings
pnpm typecheck && pnpm test                             # 前端类型检查与单元测试
pnpm exec playwright install webkit && pnpm test:e2e    # E2E（mock 模式，WebKit）
```

- `E2E_PORT=<端口>`：E2E 的 mock 开发服务器端口（默认 1421），并行跑多份 E2E 时各用一个。
- `CV_FIXTURE_DENYLIST="词1,词2"`：fixture 脱敏检查额外拦截的私有词（如公司、客户名），不写进仓库。
- 真实数据探针（只读，索引写入临时目录），用于检查性能与兼容性：

```sh
CV_REAL_ROOT=~/.claude cargo test -p cv-core --release --test assemble -- --ignored --nocapture   # 组装全部 Session
CV_BENCH_ROOT=~/.claude cargo test -p cv-core --release --test search -- --ignored --nocapture    # 建全文索引并测查询耗时
```

内置价格快照（`crates/transcript/src/pricing-snapshot.json`）的刷新方式：

```sh
curl -sSf https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json \
  | cargo run -p transcript --example update_pricing
```

`crates/cv-core/tests/fixtures/` 是从真实 Session 精简、脱敏得到的结构样本（见其中的 README），由 `crates/cv-core/examples/sanitize.rs` 生成。

## 构建

```sh
pnpm tauri build --bundles app --target aarch64-apple-darwin
```

产物在 `target/aarch64-apple-darwin/release/bundle/macos/Claude Viewer.app`，使用 ad-hoc 签名、未公证；首次打开如被 Gatekeeper 拦截，右键选择「打开」。

安装到「应用程序」（先退出正在运行的旧版本）：

```sh
ditto "target/aarch64-apple-darwin/release/bundle/macos/Claude Viewer.app" "/Applications/Claude Viewer.app"
```

在 macOS 27 上，`tauri build` 设置的 `MACOSX_DEPLOYMENT_TARGET=14.0` 会让 cargo strip 后的 proc-macro 动态库无法加载（`mis-aligned LINKEDIT string pool`），因此 `Cargo.toml` 中对 `profile.release.build-override` 关闭了 strip，不影响最终应用。

## 结构

```
crates/transcript/ 解析库：容错解析、对话组装、显示模型、进程文件、价格（ccrpc 也用它）
crates/cv-core/   索引与查询：扫描、SQLite 索引（FTS5 trigram）、搜索、统计、诊断
crates/ccrpc/     ccrpc：以 JSONL 驱动 ToH clay 里的 Claude Code（见其 README）
src-tauri/        Tauri 外壳：命令与事件、后台 worker、FSEvents 监听、Live 轮询、设置
src/              React 前端：features/* 各功能模块，ipc/ 为 IPC 层（real / mock 双实现）
e2e/              Playwright 用例
```

## 许可证

Copyright (C) 2026 XGFan

本项目以 [GNU Affero General Public License v3.0 或更高版本](LICENSE)（AGPL-3.0-or-later）发布。你可以自由使用、修改和分发；分发修改版，或以网络服务形式向他人提供修改版时，须以同一许可证公开对应源码。
