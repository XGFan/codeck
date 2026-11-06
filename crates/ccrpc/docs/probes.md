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
- 忙时输入（2.1.317 复测）：Enter 仍是入队、在下一次工具调用结束时 `absorbed_mid_turn`，入队即触发 UserPromptSubmit（`prompt_id` 为当前一轮）；等待期间 TUI 在对话里显示 `❯ <正文>` 与 `ctrl+enter to send now`，输入框占位提示 `Press up to edit queued messages`。本轮已没有工具调用可夹带时，排到本轮结束后自成一轮。`ctrl+enter`（`ESC[13;5u` 或 `ESC[27;5;13~`）是**中断本轮并立即发出**：本轮 `aborted`，该条 `dequeue` 后作为新提示词自成一轮。`↑` 把排队的全部取回输入框（`operation: "popAll"`），再 `ctrl+u` 即撤销。
- 轮次交界处的输入：Stop 之后 Claude Code 立即处理自己队列里的东西（如本轮中途结束的后台任务的通知，Stop 前已 enqueue），进程文件可能只闪一下 idle（实测 5ms）。这时粘贴回车的提示词进队列，enqueue **不带 content**，并且一直等到通知那一轮结束才送达（实测 7 分钟，期间夹着多次工具调用也没被吸收）。
- Subagent Run：`<sessionId>/subagents/agent-<id>.jsonl`，同名 `.meta.json` 的 `toolUseId` 指向主线的 `Agent` tool_use；`requestShape: "background"` 时主线的 Agent tool_result 立即返回（DeepSeek 下模型默认就这样派 Subagent：主线一轮先结束，Subagent 完成后主线再起一轮汇报）。
- Workflow Run：状态在 `<sessionId>/workflows/<runId>.json`（运行中整份重写），或只有 `subagents/workflows/<runId>/journal.jsonl`；其中的 Subagent Run 是 `subagents/workflows/<runId>/agent-<id>.jsonl`。主线 `Workflow` 的 tool_result 条目 `toolUseResult.runId` 指向它。
- rewind：输入框为空时按两次 `Esc` 打开选择器（标题 `Rewind`，提示词从旧到新排列，最后一行是 `❯ (current)`，底部 `Enter to continue · Esc to cancel`）；`↑` 选上一条、回车后出现确认菜单，其中 `Restore conversation` 只回退对话。回退后新提示词的 `parentUuid` 指向所选提示词之前的条目，`last-prompt` 的 `leafUuid` 在新提示词之后才更新。
- rewind（2.1.318 复测）：选择器每项是提示词的第一行（多行的加 `…`，长行在屏幕边截断）与下一行这一轮改了什么（`No code changes` / `w.txt +1 -1`）；除提示词外还列出插话成的轮、`/add-dir <目录>`、`/compact`（`⚠ No code restore`）、`! echo bang`，带图的写成 `[Image #2]with image`（编号在整个 Session 里递增），`/context` 不列；48 行高时只画 4 项，`↑ N more` / `↓ N more`，`↑` 到顶不再动。回车后的确认页引用这条提示词（最多前 4 行，长行折行、长词断开，末行 `(5s ago)`），写明代码会怎样（`The code will be restored -1 in w.txt.` / `The code will be unchanged.`）；有代码改动时菜单为 `Restore code and conversation` / `Restore conversation` / `Restore code` / `Summarize from here` / `Summarize up to here` / `Never mind`（48 行高时最后一项挤到下面，第 5 项显示为 `↓ 5.`），下方 `⚠ Restored files go back whole, losing later changes by anyone. Bash changes to other files are not undone.`；没有时为 `Restore conversation` / `Summarize from here` / `Summarize up to here` / `Never mind`。数字键直接选；确认页 `Esc` 退回选择器（光标不变），再 `Esc` 关掉。恢复对话后这条提示词回到输入框，transcript 要等下一条提示词才体现；恢复代码时 Write 新建的文件被删掉、改过的文件整份恢复；只恢复代码时对话与输入框都不变。进程文件在选择器与确认页期间为 `waiting` / `dialog open`。
- rewind 到命令（2.1.318）：选择器与确认页把斜杠命令写作 `/名字 参数`（项目自定义命令 `/greet world`、`/add-dir <目录>`），`!` 命令写作 `! echo bang`，与 transcript 的 `<command-name>` / `<bash-input>` 一致。恢复对话后，斜杠命令回到输入框（`❯ /greet world`）；`!` 命令回到输入框时是 shell 模式：输入框那行以 `!` 代替 `❯`（`! echo bang`，下方 `! for shell mode`），`ctrl+u` 清空后仍是 `!`，空的 shell 模式里按退格回到 `❯`。普通模式下粘贴以 `!` 开头的文字同样进 shell 模式（回车即运行）。
- `!` 命令（2.1.318）：粘贴 `! echo pasted` 后输入框为 shell 模式的 `!  echo pasted`，transcript 记为 `<bash-input> echo pasted</bash-input>`（开头多一个空格）、`<bash-stdout>`；运行后 Claude Code 把命令与输出发给模型，模型回一条，Stop hook 与 `turn_duration` 照常，没有 UserPromptSubmit（进程文件 busy → idle）。长命令运行中显示 `⎿  Running…`、模式行 `esc to interrupt`，输入框回到 `❯`。命令很快时 Stop 可能先于 transcript 里的 `<bash-input>` 到。
- 输出太大的工具结果（约 30KB 起）存进 `<sessionId>/tool-results/<id>.txt`，tool_result 只留 `<persisted-output>` 包着的说明与约 2KB 预览，模型看到的也只有这些。

## 写入

- bracketed paste 正文 + 单独的 `\r` 即提交；多行不会提前提交。输入框把长文本折叠成 `[Pasted text #1 +3 lines]`，提交后记录的是全文。部分账号（服务端开关 `tengu_virtual_pancake`，隔离的配置目录下没有）会把折叠的粘贴包成 `<pasted_content id="1b44">\n全文\n</pasted_content id="1b44">`（id 为 4 位十六进制，开头与结尾的标签都带），带图时在 `[Image #n]\n\n` 之后（2.1.317 实测；102 字符 3 行没折叠的没包）。
- 输入框为空时可能显示暗色（SGR 2）的占位提示，如 `❯ Try "edit <filepath> to..."`：新 Session 还没发过消息、且提示词建议开着时出现（真实账号默认开着；`CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=1` 强制打开，`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` 下没有）。Claude Code 自己画的光标是反显格，落在占位提示第一个字上（那一格不是暗色）；`ctrl+u` 清不掉它，读输入框内容时要跳过（2.1.317）。
- `ctrl+u` 清空单行输入框。`Esc` 中止忙状态约 400ms 后回 `idle`，**被中止的提示词会放回输入框**——下一次 `prompt` 前要先清空。
- 斜杠命令：直接写 `/clear`、`/model X` 文本再回车即可执行（`/model` 会先发一次请求校验模型，失败时屏幕显示 `model not changed`）。
- shift+tab 依次切换权限模式：manual → accept edits → plan → auto → manual。2.1.316 新配置目录的默认是 auto（分类器自动审批，不弹审批框）。

## 文字与图片穿插（2026-11-06，2.1.318）

环境：Claude Code 2.1.318，mock 模型，隔离的配置目录。

- bracketed paste 一个图片路径时，`[Image #n]` 插在**光标处**；按顺序逐项粘贴文字与图片，输入框是 `AAA [Image #1] BBB [Image #2] CCC`。transcript 的 `user` 条目记一个文字块 `"AAA [Image #1] BBB [Image #2] CCC"`，后面是各张图片（按粘贴编号从小到大，按顺序粘贴时即文字里的顺序），另有 `imagePasteIds`。忙时插话相同：队列里显示穿插的文字，`queued_command` 附件带着文字与图片。
- **必须一项一项粘贴**：几次粘贴连着写，图片落到了最后（路径是异步解析的）。每段文字要等输入框显示出来、每张图片要等多一个 `[Image #n]` 且路径离开输入框，再写下一项。
- `n` 不可预料：整个 Session 共用一个计数，`[Pasted text #n]` 也占号，清掉的占位符也用掉号。Claude Code 不在占位符前后加空格。
- 输入框以 `/` 开头时按斜杠命令处理，后面的图片随之变化：项目命令（`/greet world [Image #1] tail`）照常运行，`<command-args>` 里带着 `[Image #1]`，图片在展开后的 isMeta 条目里；不是命令的（`/tmp/foo 看这个 [Image #3]`）作为字符串提示词记下，**没有图片块**（图片丢了，只剩 `[Image: source: …]` 记录）。图片在前（`[Image #2]/greet bob`）则是普通的带图提示词。
- 同一个图片路径粘贴两次得到两张图片（`imagePasteIds: [6, 7]`），不去重。
- 长的文字段在输入框里折叠成 `[Pasted text #k +N lines]`，记录的是全文；部分账号的 `<pasted_content id=…>` 包裹出现在图片之间时是什么样子没有实测。

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
- Workflow 工具的审批框（160×48 实测，`tests/fixtures/screens/permission_workflow.txt`）：标题 `Run a dynamic workflow?`，下面是脚本名与各阶段说明（阶段本身带 `1.` 这样的编号），最后才是菜单 `❯ 1. Yes, run it` / `2. View raw script` / `3. No`，进程文件同样是 `waitingFor: "permission prompt"`。

## computer use 的授权框（2.1.318）

`mcp__computer-use__request_access` 运行中弹出（51×48 实测，屏幕存在内置布局 `layouts/e1960c70f912a9ff.json`）：

```
 Computer Use wants to control these apps

   <request_access 的 reason>

     ◉ ToH
     ◉ System Settings
       ⚠ can change system settings
     … +4 more apps not shown, and not included
   in this grant.

   ❯ Deny, and tell Claude what to do differently
     (esc)
     Allow for this session (4 apps)

  Enter to confirm · Esc to cancel
```

- 菜单没有编号：↑ / ↓ 移动 `❯`，两头循环，应用列表不能选中；回车确认，`Esc` 即 Deny。默认光标在 Deny。
- 进程文件 `waitingFor: "permission prompt"`。插件只报 PreToolUse，没有 PermissionRequest；bypass 与 manual 模式都直接弹这个框，前面没有工具审批。
- Deny：工具返回空授权（`allowedApps` 为空），PostToolUse 照常，本轮继续（不像工具审批的 No 那样中断）。Allow：本 Session 内获准，同一应用再申请不再弹框；获准的 Session 持有 `~/.claude/computer-use.lock`，直到它退出。
- 窄屏只列得下部分应用，其余写明 `not shown, and not included in this grant`，按钮上的数字是这次真正授权的应用数。

## 模型起草的产品反馈（2.1.318）

模型调用 `SendFeedback` 后，草稿只存在本机，本轮照常结束（idle），输入框上方出现一张卡片：

```
╭──────────────────────────────────────────────╮
│ ✻ Product feedback drafted: <标题>           │
│ │ - What happened: …                         │
│ │ - Repro: …                                 │
│ 1 to review · 2 to send · 0 to dismiss       │
╰──────────────────────────────────────────────╯
```

- 不是阻塞的对话框：照常能发提示词，卡片跨轮次留着，直到按键处理；本轮进行中卡片隐藏，回到 idle 再显示；模式行末尾另有 `· 1 feedback draft`。80×24 的 Pane 里卡片把输入框挤出屏幕（此时认不出输入框，发不了提示词）。
- 粘贴（bracketed paste）进输入框的数字是文字：提示词 `1` 照常发出，不会打开 `/feedback`；只有按键才是卡片的快捷键。
- 输入框为空时：`1` 等于 `/feedback`，打开草稿面板（本 Session 与其他 Session 的草稿、`+ Write new feedback`；❯ 选择，Enter 查看，`d` 立即删除、不再确认，Esc 关面板，卡片随之消失）；`0` 关掉卡片，草稿仍在 `/feedback` 里（30 天后过期），之后的轮次不再出现；`2` 发给 Anthropic（未实测）。

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
- 2.1.318：题目画在 `│ ` 引用条里（每行开头），中文折行处不加空格；按键与上面相同（`mock_model_ask_before_attach` 实测单题与两题）。提问出现时没有 ccrpc 连着，插件的 hook 无输出返回，对话框照常等人作答。
- Chat about this（2.1.318，`mock_model_ask_chat`）：每题的选项下有分隔线和编号的 `Chat about this`（单选在 `Type something.` 之后，多选在 `Submit` 行之后），按编号即生效；结果记为拒绝的 tool_result（`The user doesn't want to proceed with this tool use. … The user wants to clarify these questions. … Questions asked:`，每题 `- "题目"` 下一行 `  Answer: …` 或 `  (No answer provided)`），屏幕显示 `User declined to answer questions`，模型接着回复，本轮继续。多题时 `right` / `left` 在题目与 Review 之间切换，没答的题可跳过；多选题勾上就算答了（不必到 Submit）。主线的 PermissionRequest hook 还在运行时，对话框不接数字键与方向键（Enter 照常选中光标所在项）；hook 无输出返回后约几百毫秒才接，紧接着按的键会丢。

## hooks 与插件（2.1.317）

环境：Claude Code 2.1.317，tmux 160×48（不经 ToH），独立 `CLAUDE_CONFIG_DIR`，manual 模式。模型换成本机按脚本回复的 Messages 接口 mock（工具调用可确定复现）；hook 是只记日志、按控制文件阻塞并回复的命令 hook；进程文件每 50ms 采样。

### PermissionRequest

- 触发：主线与 Subagent 里要审批的工具调用、ExitPlanMode、AskUserQuestion（PreToolUse 没给决定时）。先 PreToolUse（带 `tool_use_id`），约 30ms 后 PermissionRequest。
- 输入：`tool_name`、`tool_input`、`permission_suggestions`（如 `{"type":"addDirectories",…}`、`{"type":"setMode","mode":"acceptEdits"}`，`destination: "session"`）、`prompt_id`、`permission_mode`；**没有 `tool_use_id`**；Subagent 里另有 `agent_id`、`agent_type`。ExitPlanMode 的 `tool_input` 是 `{}`。
- 主线：hook 运行的同时审批框照常显示，进程文件照常 `waiting`（`permission prompt`；AskUserQuestion 为 `input needed`）。hook 回 `allow` / `deny` 后对话框立即收起，屏幕显示 `Allowed by PermissionRequest hook` / `Denied by PermissionRequest hook`。`deny` 的 `message` 作为 `is_error` 的 tool_result 交给模型，本轮继续（在审批框里选 No 则中断本轮）。
- 人先在审批框里作答：选 Yes 时 hook 进程不被收掉，继续阻塞到它自己退出（1 次实测）；选 No 或按 Esc 时 hook 收到 SIGTERM。
- 后台 Subagent：hook 阻塞期间**不显示**审批框，进程文件保持 `busy`；hook 退出且没给决定后审批框才出现在主屏（提示行多出 `ctrl+x ctrl+k twice to stop background agents`），进程文件变 `waiting`。hook 回 `allow` 生效。
- ExitPlanMode：hook 回 `allow`（带不带 `updatedPermissions`）都被忽略，计划审批框照旧等人作答；回 `deny` 生效。PreToolUse 回 `allow` 也照样弹出计划审批框。→ 计划审批的“同意”只能在屏幕上作答。mock 下的计划审批框是 `Exit plan mode?` / `1. Yes, and switch to default (ask each time) for this session` / `2. No`。
- Notification `permission_prompt` 在审批框出现约 6s 后才触发。

### AskUserQuestion

- PreToolUse 阻塞期间不显示提问界面，进程文件是 `busy`，spinner 行显示 `running PreToolUse hook`。
- PreToolUse 回 `permissionDecision: "allow"` 与 `updatedInput`（原样的 `questions` 加 `answers`）即作答，界面不出现。
- PreToolUse 不给决定时接着触发 PermissionRequest，这时提问界面与 hook 同时出现；PermissionRequest 回 `behavior: "allow"` 与同样的 `updatedInput` 也能作答，屏幕显示 `User answered Claude's questions:` 与 `Allowed by PermissionRequest hook`。
- `answers` 以题目文字为键。单选写 label；多选写 `"eslint, clippy"` 或 `["eslint","ruff"]`（数组在结果里以 `, ` 连接）；Other 直接写自由文本。
- tool_result：答案都是选项时为 `Your questions have been answered: "Q"="A". You can now continue with these answers in mind.`（与在界面里作答相同）；有选项之外的答案时为 `The user answered: "Q"="A", …. Read the answers carefully — they may request clarification, changes, or that you not proceed — and follow what they actually say.`

### 中止与轮次结束

- 正常结束：Stop 带 `last_assistant_message`（本轮最后一段 assistant 文字）、`stop_hook_active`。
- Esc 中止流式回复：Stop 与 StopFailure 都不触发。审批框期间按 Esc 同样不触发 Stop。
- API 报错（mock 返回 400）：只触发 StopFailure，`error: "unknown"`，`last_assistant_message` 为 `API Error: 400 probe error`；不触发 Stop。
- 后台 Subagent：SubagentStart（`agent_id`、`agent_type`）→ 主线一轮先结束、触发 Stop（屏幕显示 `Waiting for 1 background agent to finish`）→ SubagentStop（带 `agent_id`、`last_assistant_message`）→ 结果通知回到主线时**也触发 UserPromptSubmit** → 再一次 Stop。

### transcript 与上下文

- 零输出的 hook 不在 transcript 里留条目，例外是 Stop：每次都写一条 `system` / `stop_hook_summary`（含 hook 命令）。hook 给出决定时写 `attachment` / `hook_permission_decision`（`decision`、`toolUseID`、`hookEvent`）；有 stdout 时写 `attachment` / `hook_success`（含 stdout 全文，如 AskUserQuestion 的答案 JSON）。
- 发给模型的请求里没有 hook 运行的痕迹（逐个检查全部 37 个请求）；hook 的决定只以工具结果出现（`deny` 的 `message`、AskUserQuestion 的答案）。
- 只含 hooks 的插件：`claude plugin details` 报 `Hooks (4) … (harness-only — no model context cost)`、`Always-on: ~0 tok`；装上前后请求的 system 与 tools 逐字相同。

### 生效时机

- 运行中用 `claude plugin install` 装的插件不生效；在 Session 里执行 `/reload-plugins` 后生效（`Reloaded: 1 plugin · … · 4 hooks …`），但这条命令连同输出会作为本地命令记录进入之后的上下文。
- 运行中直接改 `settings.json` 的 `hooks`，下一条提示词就生效，不用 reload。
- `claude --plugin-dir <插件目录>` 只给这个进程加载插件，其 hooks 生效；`--session-id <uuid>` 指定的 id 同时出现在 hook 输入与进程文件里。

### 其他

- hook 进程的父进程就是 claude 进程（进程文件名里的 pid）。
- hook 输入都带 `session_id`、`transcript_path`、`cwd`；除 SessionStart 外还有 `prompt_id`、`permission_mode`（`default` / `plan` / `acceptEdits`）。
- `/clear`：回车后约 40ms SessionEnd（`reason: "clear"`），随即 SessionStart（`source: "clear"`，带新的 `session_id` 与 `transcript_path`）；进程文件的 `sessionId` 在回车后约 90ms 已换新，与上面 2.1.316 记的约 1.3s 不同。

### 补测（实现前）

- **超时**：PermissionRequest hook 设 `timeout: 700`，阻塞到 663s 仍在，723s 前被 SIGTERM（超过缺省 600s 的值生效）；超时后审批框仍在、进程文件仍 `waiting`，人照常可答。
- **插话（steer）**：忙时输入在入队时触发 UserPromptSubmit，`prompt` 为插话正文，`prompt_id` **与当前一轮相同**；本轮只有一个 Stop。transcript 的用户条目带 `promptId`。
- **斜杠命令与 `!` 命令**：`/status`、`/model …` 不触发任何 hook（`/model` 弹出 `Switch model?` 确认框；`/add-dir <目录>` 弹出 `Add directory to workspace` 确认框，进程文件为 `dialog open`，2.1.318 实测）；`!echo bang` 不触发 UserPromptSubmit，但触发一次 Stop（新的 `prompt_id`，`last_assistant_message` 为空），transcript 里是同一 `promptId` 的 `<bash-input>` / `<bash-stdout>`。
- **“总是允许”**：Bash `touch` 的审批框第 2 项是 `Yes, and always allow access to <cwd> from this project`；`permission_suggestions` 为 `addDirectories`（`destination: "session"`）与 `setMode acceptEdits`。在 TUI 选第 2 项不写任何 settings 文件、不切模式（仍 manual），下一次同样的 `touch` 仍要审批。
- **deny 加 `interrupt: true`**：与在审批框选 No 相同——显示 `Interrupted · What should Claude do instead?`，回 idle，不触发 Stop。
- **Workflow**：`Run a dynamic workflow?` 审批同样触发 PermissionRequest（`tool_input` 只有 `script`，没有 `permission_suggestions`），审批框与 hook 同时出现；hook 回 `allow` 生效。
- **审批框刚出现时丢掉按键**：后台 Subagent 的审批框（标题 `Bash command · from the general-purpose agent`，提示行多 `ctrl+x ctrl+k twice to stop background agents`）一出现就按 `1` 不生效，审批框原样还在；隔 200ms 以上再按就生效（主线仍在流式输出也一样）。→ 按键作答前等审批框出现满 500ms，按后 2 秒内没离开且仍是同一请求就再按一次。

### 补测（全部迁移前）

- **后台任务的启动**：PostToolUse 的 `tool_response` 里，后台 shell 有 `backgroundTaskId`（`tool_input` 有 `command`、`description`）；后台 Subagent 有 `isAsync: true`、`status: "async_launched"`、`agentId`、`description`；Workflow 有 `taskId`、`taskType: "local_workflow"`、`workflowName`、`runId`、`summary`。SubagentStart（`agent_id`、`agent_type`）前台、后台 Subagent 都触发。
- **后台任务的结束**：结果通知是一条 UserPromptSubmit，`prompt` 为 `<task-notification>`（`<task-id>`、`<status>`、`<summary>`）。主线空闲时立即送达、自成一轮；主线忙时等当前工具结束被吸收，`prompt_id` 为当前一轮的（同插话）。状态：自然结束 `completed`（`Background command "…" completed (exit code 0)`、`Agent "…" finished`），在 `/tasks` 面板按 `x` 停掉 `killed`（`Agent "…" was stopped by user`，停掉后立即送达）。SubagentStop 只在 Subagent 自己结束时触发（带 `agent_transcript_path`、`last_assistant_message`），被停掉时不触发。
- **Stop 的 `background_tasks`**：每项 `id`、`type`（`subagent` / `shell` / `workflow` / `teammate`）、`status`、`description`，shell 另有 `command`，Subagent 另有 `agent_type`，Workflow 另有 `name`。teammate（2.1.318，Agent 带 `name`）：工具结果是 `status: "teammate_spawned"`、没有 `isAsync`；Stop 里的 `id` 是另一个任务 id（不是 agentId），每次空闲触发 SubagentStop、收到消息再 SubagentStart，Stop 里一直是 `running`。Subagent Run 里启动的后台 shell，PostToolUse 带该 Subagent 的 `agent_id`，结果通知送给那个 Subagent，主线收不到。
- **“总是允许”与 hook**：`python3 -c 'print(42)'` 的审批框第 2 项是 `Yes, and don’t ask again for: python3 *`，选它写入项目的 `.claude/settings.local.json`：`Bash(python3 *)`；而 `permission_suggestions` 是 `addRules` 精确命令（`destination: "localSettings"`），经 hook 回填写入的是 `Bash(python3 -c 'print\(42\)')`。两者不同。→ `allow_always` 按审批框自己的选项作答，再放掉扣着的 hook。
- **Stop 之后才送达的插话**：主线只在流式输出文字（没有工具调用）时插话，入队时触发 UserPromptSubmit（旧的 `prompt_id`），本轮 Stop 之后它作为新的一轮运行：**没有自己的 UserPromptSubmit**，只有一个带新 `prompt_id` 的 Stop（两个 Stop 几乎同时）。transcript 里这一轮的 `turn_duration` 从插话提交时算起。

### 补测（GUI MVP 前）

- **`claude --resume <id>`**：不换 sessionId——进程文件与 SessionStart（`source: "resume"`）都是原来的 id，transcript 仍是同一个文件。
- **Pane 的生命周期**：`toh new -- 命令` 把命令敲进 Pane 的登录 shell 里运行，命令退出后 Pane 留在 shell 提示符（Holder 仍在）。要结束 Pane，再在 shell 里 `exit`。
- **目录信任框**（在没信任过、也不在已信任目录之下的目录里启动；已信任目录的子目录不弹）：`Accessing workspace:` / 路径 / `Quick safety check: Is this a project you created or one you trust? …` / `❯ No, exit` / `Yes, I trust this folder` / `Enter to confirm · Esc to cancel`。菜单不带编号，光标初始在 No，方向键移动、回车确认（`tests/fixtures/screens/trust_folder.txt`）。期间没有进程文件、不触发 SessionStart；信任后才有 SessionStart（`startup`）与进程文件。
- **进度行**：输入框上方 `<符号> <动词>… (<用时> · ↓ <n> tokens)`，符号轮换 `✻ ✶ ✳ ✢ · ✽`，动词每轮随机（`Deliberating`、`Scampering`…）；刚开始时没有括号（`Considering…`），hook 运行时括号里先有 `running <事件> hook`（`Warping… (running Stop hook · 0s)`）；没有 `…` 的是别的状态行（`Waiting for 1 background agent to finish`、`Baked for 0s · done …`）。
- **启动后的时序**：进程文件出现（idle）时 TUI 可能还没画出输入框，要等屏幕上出现输入框再写入。

## 转入后台（agents 页面，2.1.318）

环境：Claude Code 2.1.318，ToH clay 120×40，独立 `CLAUDE_CONFIG_DIR`、`TOH_STATE_DIR`、`CCRPC_STATE_DIR`，插件经 `--plugin-dir`，mock 模型；经 ccrpc 的 `send_keys` / `get_screen` 操作。Pane 里的 claude 记作 P，原 Session 记作 OLD；后台 claude 记作 B，它的 Session 记作 NEW。

- **daemon 按配置目录隔离**：后台 claude 由 `claude daemon run --origin transient --spawned-by {"label":"claude","cwd":…,"pid":P}` 管理，socket 在 `/tmp/cc-daemon-<uid>/<配置目录路径的 sha256 前 8 位>/`（`~/.claude` 是 `afcfaf26`，测试的配置目录各不相同），状态在 `<配置目录>/daemon/`（`roster.json`、`attach-journal/`）。测试的 daemon 碰不到主人的。P 还在时 daemon 被杀会由 P 重新拉起；daemon 与它的 `bg-pty-host` 在 P 退出后仍在，收到 SIGTERM 后几秒才退出。
- **`←` 即转入后台**：输入框为空时按 `←`（idle 或回复进行中都行）立即转入后台，不需要再选、回车（真实事故里人是 `←` 后选中回车）。daemon 或认领事先起好的备用进程（`claude bg-pty-host … 200 50 -- claude --bg-spare …`），或直接起 `claude --bg-pty-host <daemon 目录>/pty/<job>.sock 120 40 -- claude --session-id NEW --fork-session --resume <OLD 的 transcript> <原参数>`；两种都实测到。daemon 另外常备 1–2 个备用进程（进程文件 `kind: "bg"`、`spare: true`、各自的 `jobId`），它们继承 P 的环境，ccrpc 插件也以同一个 Holder 报到（`source: "startup"`）。
- **agents 页面**（`tests/fixtures/screens/agents_list.txt`）：顶部 `Your conversation moved to the background — enter opens it · esc returns to it · ctrl+c twice quits`，下面 `Needs input` / `Working` / `Completed` 三段列出后台 Session；底部是两条横线夹着的 `❯ describe a task for a new session`，**与输入框同形**，在里面输入回车会新开一个后台 Session（认领备用进程），提示行 `enter to return · space to reply · ctrl+x to delete · ? for shortcuts`（选中别的 Session 时 `enter to open`，选中收起的分段时 `enter to expand · ctrl+x to delete all`）。纯文字里看不出选中哪一项。
- **转发的界面**：回车或 Esc 打开 B 后，P 原样转发 B 的界面（`tests/fixtures/screens/background_session.txt`）：输入框、模式行 `⏸ manual mode on · ← for agents`、审批框都与平常相同，现有布局都认得。经 Holder PTY 写的按键与 bracketed paste 都送到 B：粘贴提示词回车后它在 B 里运行、记进 NEW。在 B 的输入框里打 `/model`、`/mo`、`/`、`/add-dir /tmp` 时，补全列表画在框上方，框下仍是模式行（`⏸ manual mode on`，`· ← for agents` 这一段不显示）；fullscreen TUI 相同（`mock_model_follows_the_background_session` 在 B 里跑 `/add-dir`、`set_model` 与提示词都正常）。
- **进程文件**：P 仍是 `sessionId: OLD`、`kind: "interactive"`，多了 `parkedJobId: "<job>"`，`status` 停在转入时的值（idle，或回复进行中时的 busy），之后不再更新。B 是 `kind: "bg"`、`jobId: "<job>"`（NEW 的前 8 位）、`sessionId: NEW`，`status` 照常。之后 `←` 回列表、Esc 回到 B、打开另一个后台 Session（Pane 改为转发它）、在列表里 `ctrl+x` 停掉 B，P 的 `parkedJobId` 都不变；Pane 正在显示哪个后台 Session，进程文件与 daemon 目录里都没有记录。
- **停掉与再打开**：`ctrl+x` 停掉 B 后 B 的进程与进程文件消失，列表把它放进 `Completed`（`stopped`）。再按 Esc（`returns to it`）会重新运行这个 job：新的 pid（认领备用进程），同样的 `sessionId` 与 `jobId`，插件以 `source: "resume"` 重新报到。
- **一个配置目录一个 daemon**：同一配置目录下第二个 Pane（P2）再转入后台时不另起 daemon，用第一个 Pane 的 P 拉起的那个；daemon 的环境是那个 Pane 的，所以 P2 的对话所在的后台 claude 以及备用进程的报到都是**第一个 Pane 的 `holder_id`**（`mock_model_follows_the_background_session_of_a_second_pane` 实测：P2 在 Holder `m50f…`，它的 B 的报到是 `m27f…`）。daemon 在拉起它的 P 退出后仍在（主人真实的 daemon 父进程是 1）。能把 P 与 B 对上的只有 P 的 `parkedJobId` 与 B 的 `jobId`。
- **报到与 hook**：B 的插件照常报到（`<ccrpc 状态目录>/sessions/<B>.json`，`holder_id` 是拉起 daemon 的 Pane 的（单个 Pane 时即 P 的），`source: "fork"`，停掉再打开为 `resume`；备用进程自己的报到是 `startup`），hook 按 B 的 pid 连 `s/<B>.sock`。连着 P 的 ccrpc 不跟随时收不到 B 的 hook，`prompt` 等 30 秒后报 `timeout`（提示词其实已在 B 里运行）。
- **transcript**：转入时 OLD 末尾写 `session_cron_carry` attachment、`cost-state`、`continued-in`（`continuedInSessionId: NEW`）。idle 时转入，NEW 的文件要等下一条提示词才出现（开头是复制来的 OLD 全部条目）；回复进行中转入，NEW 立即出现：OLD 只有那条提示词、没有回复，B 写一条不带 `promptId` 的 `<system-reminder>Your previous response was interrupted mid-generation…` 用户条目，接着把回复写完，照常有 Stop（`stop_hook_summary`）与 `turn_duration`，没有 UserPromptSubmit；屏幕显示 `Continuing an interrupted response.`。
- **挂着审批 / 提问时**：审批框与 AskUserQuestion 都吃掉 `←`，界面不变、不转入后台；要先作答。B 里的审批框也一样。
- **退出**：在 B 的界面里输入 `/exit` 回到 agents 列表，B 照常运行；B 的界面里 `ctrl+c` 两次不退出。在 agents 列表里 `ctrl+c` 两次 P 退出，Pane 回到 shell（ccrpc 发 `session_exit`），B 留在 daemon 里继续运行、没有 Pane 显示它。

## Fork（`--resume --fork-session`，2026-11-06，2.1.318）

- `claude --resume <id> --fork-session` 起来后插件以 `source: "fork"` 报到（新 Session 的 id，transcript 路径指向还不存在的文件）；回退或 `/rename` 之前不写任何文件，此时退出什么都不留。
- 回退（只恢复对话）后才写出新 transcript，只含回退点之前的历史；复制来的条目 uuid 与原 Session 相同，**没有 `forkedFrom`**。`last-prompt`、`file-history-snapshot` 里仍可能提到回退掉的那条提示词的 uuid。
- 文件历史随之复制（`copyFileHistoryForResume`）：Fork 里回退到改过文件的提示词之前，确认页照样有「Restore code and conversation」与 ⚠ 警告，恢复的是工作目录里的文件（与原 Session 共用）。
- `/rename` 记成本地命令（`system` / `local_command` 条目，接在 Main Line 末尾）并写 `custom-title` / `agent-name`。不回退直接 `/rename` 时整段历史分几次写出（先 `custom-title`，再是复制来的条目），所以要等文件长度稳定才算写完。
- 选择器与确认页上的时间（`5s ago`）是原提示词的发送时间，与原 transcript 一致。

