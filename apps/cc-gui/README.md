# cc-gui

基于 [ccrpc](../../crates/ccrpc) 的 Claude Code 桌面 GUI（macOS，只在本机用）：在一个窗口里看本机 ToH clay 里所有 Claude Code Session，发提示词、插话、排队、答审批和提问、看 Subagent 过程。TUI 同时在 ToH Mac Client 里显示（共显），GUI 处理不了的界面回 TUI 处理。目的是在日常开发里验证 ccrpc，规格见 [docs/spec.md](docs/spec.md)。

## 前提

- ccrpc：在仓库根目录 `cargo build --release -p ccrpc && cp target/release/ccrpc ~/.local/bin/`，再 `ccrpc plugin install`（已在运行的 claude 要重启才加载插件，没加载的 Session 在侧栏里是灰的）。
- PATH 里有 `toh`、`claude`（取自登录 shell：GUI 启动时跑一次 `$SHELL -l -c env`，从 Dock 启动也一样）。
- 想让认不出的界面先经 AI 分析：在 shell 环境里设 `CCRPC_ANALYZER_API_KEY`（可选 `CCRPC_ANALYZER_BASE_URL`、`CCRPC_ANALYZER_MODEL`）。
- 用哪个 ccrpc：环境变量 `CC_GUI_CCRPC`，否则 `~/.local/bin/ccrpc`，否则 PATH 里的。换了 ccrpc 的二进制后，在 Session 菜单里点“重连 ccrpc”。

## 构建与运行

```sh
pnpm install
pnpm tauri build          # → src-tauri/target/release/bundle/macos/cc-gui.app，拖进 /Applications
pnpm tauri dev            # 开发
pnpm dev:mock             # 只在浏览器里看界面：后端换成 e2e/fixtures 的回放
```

## 用法

- 侧栏每 2 秒刷新本机 clay 里的 Session，等作答的置顶（琥珀色点）；点开才连接（`ccrpc attach --co-display`），连上后一直连着。“新建”选一个目录，经 `ccrpc new` 在 clay 里起一个 claude。
- 输入框：空闲时 Enter 发送；忙时 Enter 排到本轮之后（`follow_up`，输入框上方可撤销），⌘Enter 立即插话（`steer`）；Esc 中止。可以粘贴截图。
- 审批卡片：允许 / 总是允许（按钮上是审批框原文，写着它会加的规则）/ 拒绝（中断本轮）/ 拒绝并告诉 Claude（本轮继续）；数字键 1–3 对应。分析器学来的界面标“AI 识别”。ccrpc 认不出的界面只显示屏幕原文，去 Mac Client 里处理。
- Dock 角标是等作答的 Session 数；离开电脑时的提醒靠 claude-away-notify，GUI 不发系统通知。
- ccrpc 意外退出时顶部出横幅，点“重连”。

## 日志与排查

每个连接一个目录 `~/Library/Logs/cc-gui/<日期>/<holder_id>-<时间>/`：`wire.jsonl`（与 ccrpc 之间的每一行原样）、`ccrpc.log`（ccrpc 的 `--log`）、`gui.log`（操作与 ccrpc 的退出、stderr）；全局的 `~/Library/Logs/cc-gui/gui.log`。保留 14 天。怎么看见 [CLAUDE.md](CLAUDE.md)。

## 测试

```sh
pnpm typecheck && pnpm test        # 单测（vitest）
pnpm test:e2e                      # Playwright（WebKit）：界面对 e2e/fixtures 的 wire 回放
cd src-tauri && cargo test         # 后端单测
pnpm fixtures                      # 从 ccrpc 的 mock 端到端重新录 tools / session / turns 三个 fixture
```

真机（不碰自己的 ToH / claude）：`scripts/sandbox/` 有独立的 `TOH_STATE_DIR` / `CLAUDE_CONFIG_DIR` / `CCRPC_STATE_DIR` 和本地 mock 模型。

```sh
python3 -I scripts/sandbox/mock_model.py 18765 &
(cd src-tauri && cargo test --test sandbox -- --ignored)   # 后端经真实 ccrpc + claude 跑一轮审批，检查三份日志
scripts/sandbox/run-app.sh                                  # 在沙盒里开 GUI（需要先 pnpm tauri build）
```
