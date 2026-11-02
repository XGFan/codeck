# 实测结论（M0）

环境：Claude Code 2.1.316（macOS，原生构建），ToH clay，PTY 80×24；模型 DeepSeek `deepseek-flash`（Anthropic 兼容接口 `https://api.deepseek.com/anthropic`）；独立 `CLAUDE_CONFIG_DIR` 与 `TOH_STATE_DIR`。按键用传统 xterm 编码（数字、方向键、`Esc`、`\r`、`ESC[Z`）全部生效。

## 进程文件

- 首次启动的 onboarding（主题、安全提示）与目录信任对话框期间**没有**进程文件，`sessions/` 下只有 `<pid>.<hash>.key`。进程文件在进入主界面后才出现，初始 `status: idle`。
- 回车提交后约 0.8–1s `status` 才变 `busy`（写入有节流）；用户消息约在同一时刻进 transcript。
- **`idle` 比本轮最后的 assistant 内容落盘早约 60–120ms**；transcript 里的 `{"type":"system","subtype":"turn_duration"}` 紧跟在最后一条 assistant 之后，是本轮结束的确切标记。
- 后台 Subagent：主线先写一条 `turn_duration`（此时仍 `busy`），Subagent 结束后以 `<task-notification>` 用户消息回到主线，再写一条 `turn_duration`，然后才 `idle`。→ `turn_end` 规则：进程文件变 `idle` 后，等到下一条 `turn_duration`（超时 2s）再发。
- 审批：`status: waiting, waitingFor: "permission prompt"`，比 transcript 写出对应 `tool_use` 早约 120ms。
- AskUserQuestion：`status: waiting, waitingFor: "input needed"`，同样早于 `tool_use` 落盘约 120ms。
- 作答后约 350ms 变回 `busy`。
- `/clear`：约 1.3s 后进程文件的 `sessionId` 换成新的，新 transcript 文件随之出现，开头是 `/clear` 的本地命令记录。

## transcript

- 每个内容块单独一行：一次 API 响应的 thinking、text、tool_use 各一条 `assistant` 条目（同一 `message.id`）。`tool_result` 是 `user` 条目，content 为数组。
- 调用方的提示词是 `user` 条目，`message.content` 为字符串，与写入的正文逐字相同（4511 字符、含中文和换行的 bracketed paste 一次写入，记录完全一致）。
- 混在主线里的非对话条目：`attachment`、`api-request*`、`mode`、`permission-mode`、`atis-latch`、`file-history-snapshot`、`last-prompt`、`cost-state`、`tools-used`、`system`（`local_command`、`turn_duration`）、`queue-operation`。
- 忙时输入：先写 `{"type":"queue-operation","operation":"enqueue","content":…}`；当前工具结束时写 `operation: "remove", reason: "absorbed_mid_turn"`，内容作为 `attachment.type = "queued_command"` 插在该 `tool_result` 之后送给模型。→ **steer 成立**，语义与 Pi 相同（在两次工具调用之间送达）。
- Subagent Run：`<sessionId>/subagents/agent-<id>.jsonl`，同名 `.meta.json` 的 `toolUseId` 指向主线的 `Agent` tool_use；`requestShape: "background"` 时主线的 Agent tool_result 立即返回。

## 写入

- bracketed paste 正文 + 单独的 `\r` 即提交；多行不会提前提交。输入框把长文本折叠成 `[Pasted text #1 +3 lines]`，提交后记录的是全文。
- `ctrl+u` 清空单行输入框。`Esc` 中止忙状态约 400ms 后回 `idle`，**被中止的提示词会放回输入框**——下一次 `prompt` 前要先清空。
- 斜杠命令：直接写 `/clear`、`/model X` 文本再回车即可执行（`/model` 会先发一次请求校验模型，失败时屏幕显示 `model not changed`）。
- shift+tab 依次切换权限模式：manual → accept edits → plan → auto → manual。2.1.316 新配置目录的默认是 auto（分类器自动审批，不弹审批框）。

## 审批框（2.1.316）

```
 Bash command
 …
 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and always allow access to
      <长路径折行>
      from this project
   3. Yes, and switch to auto mode · auto mode handles these prompts for you
   4. No

 Esc to cancel · Tab to amend
```

- 按选项编号的数字键立即生效，不需要回车。
- 编号随工具与场景变化（例如是否有“切到 auto 模式”一项），**必须按文字识别**：`Yes` → allow；以 `Yes, and always allow` / `Yes, and don't ask again` 开头 → allow_always；`No`（可能带后缀）→ deny。
- 选 No：本轮中断，屏幕显示 `Interrupted · What should Claude do instead?`，状态回 `idle`。

## AskUserQuestion（2.1.316）

多题时顶部有标签行，最后一个是 Submit：

```
←  ☐ Indent  ☐ Linters  ✔ Submit  →

Indent style?

❯ 1. Tabs
     Indent with tab characters.
  2. Spaces
     Indent with space characters.
  3. Type something.
────────────────
  4. Chat about this

Enter to select · Tab/Arrow keys to navigate · Esc to cancel
```

- 单选：按选项数字立即选中并跳到下一题（标签变 `☒`）。
- 多选：选项带 `[ ]` / `[✔]`，数字键切换勾选；题内另有一行不带编号的 `Submit`；用 `right` 进下一个标签。
- 最后的 Submit 标签显示 `Review your answers` 与 `1. Submit answers` / `2. Cancel`，按 `1` 提交。
- 只有一题时没有 `←`/`→` 和 Submit 标签（只有 ` ☐ Header`），选中即提交。
- Other：按 `Type something.` 的编号进入输入状态（提示行出现 `ctrl+g to edit in Vim`），写入文字后回车。多选题里是 `[ ] Type something`。
- 结果记录为 tool_result：`Your questions have been answered: "Q1"="A1", "Q2"="a, b". …`，多选答案以 `, ` 连接。
