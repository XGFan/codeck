# cc-gui 规格（v0）

cc-gui 是基于 [ccrpc](../../ccrpc) 的 Claude Code 桌面 GUI，只在本机用。目的是验证 ccrpc 能否支撑日常开发：在日常使用里暴露 ccrpc 的缺口，据此改进它的稳定性与适用性。术语沿用 ccrpc 的 [CONTEXT.md](../../ccrpc/CONTEXT.md)。

## 1. 边界

- 内容只来自 ccrpc（[ADR-0001](./adr/0001-content-only-from-ccrpc.md)）：`get_messages`、`block`、`branch` 等协议消息；不读 transcript、进程文件，不用 cv-core。
- 每个点开的 Session 连一个 `ccrpc attach --co-display` 子进程（新建用 `ccrpc new --co-display --cwd DIR`）；TUI 同时在 ToH Mac Client 里显示，GUI 处理不了的在 TUI 里处理。
- 验证指标：被迫回 TUI 的次数（靠使用体感，不做自动统计）。
- 技术栈同 claude-viewer：Tauri 2 + React 19 + Vite。渲染组件从 claude-viewer 复制后改成吃 ccrpc 的 block，不抽共享包。Rust 后端只管 ccrpc 子进程、转发 JSONL、写 wire / gui 日志；协议解析与状态在前端。
- ccrpc 可执行文件：缺省 `~/.local/bin/ccrpc`（与 `ccrpc plugin install` 时的路径一致），设置里可改；换了二进制点“重连”生效。

## 2. 环境

- 启动时用登录 shell 跑一次 `$SHELL -l -c env`，作为所有 ccrpc 子进程的环境（PATH 里的 `toh`、`claude`，`CCRPC_ANALYZER_*`，`CLAUDE_CONFIG_DIR` 等）。密钥只来自环境，不进 GUI 设置。
- 启动后跑 `ccrpc doctor`：`fail` 在顶部显示一条，`warn` 只记 gui.log；设置页单独显示分析器是否开启。

## 3. Session 列表

- 每 2 秒 `ccrpc ls --json`，侧栏列出全部；没报到插件的置灰（提示重启 claude）。状态点取 `status`。
- 点开才连，连上后保持到 Session 退出或 GUI 关闭。
- 新建：选目录 → `ccrpc new --co-display --cwd`（v0 不带额外 claude 参数）。
- 提醒（不发系统通知，离开时靠 claude-away-notify）：等作答的 Session 琥珀色实心点并置顶；`turn_end` 后没看过的有未读点（只有连着的 Session 有）；Dock 角标为等作答的 Session 数；切回 GUI 时当前 Session 无事而别处在等，顶部提示一行，点击跳转。

## 4. 主视图

| 区域 | 内容 | 来源 |
|---|---|---|
| 顶栏 | 标题、cwd、模型、权限模式（可点，`set_mode`）、上下文 token | `ready` / `session_info`，标题取 `ls --json` |
| 消息流 | 按轮分组；文字 Markdown，工具折叠摘要（Edit diff、Bash 输出），thinking 默认折叠 | `get_messages` 首屏最新 200 块，往上滚 `before` 翻页；实时 `block` 追加；`branch` 删掉 `at_uuid` 之后的块 |
| 轮末 | 用时、token、估算花费；错误；已中止 | `turn_end` |
| 输入框上方 | 进度行；排队的 follow_up（可撤销） | `progress`、`get_state.follow_ups` |
| 右侧面板 | Agent / Workflow 调用下的 Subagent Run 过程、Workflow 阶段 | `get_messages {subagents}`、带 `agent_id` 的 `block`、`workflow` |
| 后台任务 | 输入框旁角标，点开列表可停止 | `get_tasks`、`stop_task` |

- 截断或落盘的工具结果展开时取 `get_tool_result`。
- 带图的提示词（user block 带 `images`）在气泡里显示缩略图，内容用 `get_image` 取。
- `session_changed`：清空，按新 Session 重新 `get_messages`。`session_exit`：输入框禁用，消息保留。
- ccrpc 意外退出：顶部横幅（退出码）+“重连”按钮，不自动重连。

## 5. 输入框

| 状态 | Enter | 其他 |
|---|---|---|
| idle | `prompt` | |
| busy | `follow_up` | ⌘Enter `steer` |
| waiting | 不发送，焦点到请求卡片 | |
| 任意 | | Esc `abort`（busy / waiting） |

- 粘贴图片：存到 `~/Library/Caches/cc-gui/images/`，输入框上方以 `data:` 地址预览（CSP 只放行 `data:` 图片），路径作为 `images` 传给 `prompt` / `follow_up`（`steer` 不带图片：带着图片按 ⌘Enter 会提示改用 Enter）。
- `/` 开头照常走 `prompt`（v0 没有补全）。
- 共显时写入会清空 TUI 输入框里没发的内容（ccrpc 已知限制，接受）。

## 6. 请求卡片

| 事件 | 卡片 | 作答 |
|---|---|---|
| `permission_request` | 工具名 + 输入渲染（Edit / Write diff、Bash 命令、其余 JSON）；后台 Subagent 标出 agent | 按 `options`：Yes / 总是允许（按钮显示审批框全文）/ No（中断本轮）；另有“No，并告诉 Claude…”（`deny` + `message`）。`options` 为 null 时不显示“总是允许” |
| `question` | 题目、单选 / 多选、“其他”文本框 | `answer_question` |
| `choice` | 标题 + 选项按钮 | `answer_choice`（按 label） |
| `dialog` / `unrecognized` | 只读等宽显示 `lines` +“在 Mac Client 里处理” | 无按键；状态离开 waiting 即消失 |

- `layout_source: "learned"` 的卡片标“AI 识别”。
- waiting 但还没有请求（分析器在分析）时显示“正在识别界面…”。
- 多个请求竖向叠放，主线在上。焦点不在输入框时数字键 1/2/3 对应第一张卡片的选项（输入框里照常打字）；作答成功即收起该卡片。Esc `abort`（卡片里的输入框中按 Esc 只离开输入框）。

## 7. 日志

常开，按 Session 分目录，0600，保留 14 天：

```
~/Library/Logs/cc-gui/<日期>/<holder_id>-<启动时间>/
  wire.jsonl   与 ccrpc 之间每一行原样（命令、响应、事件，block 全文）：{t: unix 毫秒, dir: send|recv, msg}；超过 50MB 轮转为 wire.1.jsonl
  ccrpc.log    ccrpc --log
  gui.log      用户操作（按钮 → 命令 id）、渲染异常、ccrpc 退出码与 stderr
```

顶栏标题旁（悬停显示）“复制排查信息”：复制 holder_id、session_id、cwd 与这个连接的日志目录，交给排查的 Session。

出问题时另开终端 Session 排查，不做应用内的问题上报。排查方法见 CLAUDE.md。

## 8. 测试

- E2E：Playwright 跑 Vite 页面，mock Tauri IPC，mock 后端按 wire.jsonl 回放（收到命令回录下的响应，事件按录下的顺序推）。覆盖：首屏与翻页；prompt → block → turn_end；审批（三个选项、带 message 的拒绝）、question、choice（含 AI 识别标签）；busy 时 follow_up / steer 与撤销；`branch`、`session_changed`；ccrpc 退出横幅与重连。
- fixture 来自 ccrpc 的 mock 端到端录下的 wire（脚本在 `../ccrpc` 跑 `cargo test --test e2e mock_ -- --ignored` 并拷入）；日常遇到的问题也截取 wire.jsonl 片段（脱敏）补进来。
- Rust 后端单测 wire 日志的写入、轮转与过期清理；`src-tauri/tests/sandbox.rs`（`--ignored`）在 `scripts/sandbox` 的沙盒里（独立的 ToH / claude / ccrpc 目录、本地 mock 模型）经真实 ccrpc 与 claude 跑一轮审批、`get_messages` 与退出，检查三份日志。

## 9. 顺序

1. ccrpc：`permission_request` 带 `options`；`choice` / `permission_request` 带 `layout_source`；同步 spec / schema / README。
2. ccrpc：mock 端到端录 wire；录出第一批 fixture。
3. cc-gui 骨架：shell 环境、侧栏、attach 子进程、三层日志、CLAUDE.md 排查一节。
4. 消息流、输入框（含图片）、请求卡片 → 开始日常使用。
5. 右侧面板、后台任务、新建 Session、提醒 → v0 完整。

并行：ccrpc `--log` 补足（响应与错误码、识别决策、整屏、版本），第 4 步之前完成。

## 10. 不在 v0

rewind、斜杠命令与模型的补全、`--resume` 等 claude 参数、在 GUI 里对认不出的界面按键、系统通知。
