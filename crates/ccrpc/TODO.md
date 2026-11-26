# TODO

## 兼容升级与协议补全（已落地，后续）

- ccrpc 插件：PostToolUseFailure / PermissionDenied 结束请求、hook 已超时后回落按键，都没有端到端用例；Claude Code 升级后先重跑 probes.md 的 hooks 一节（hook 输入字段、阻塞时界面是否照常显示）
- 学到的布局只在本机：常见界面学到后用 `ccrpc layouts export .` 导出成内置布局，提交前检查其中的整屏文字
- 分析器只处理“等待作答的对话框”；输入框、模式行、后台任务面板仍是代码里的识别，Claude Code 改了这些界面要改 recognize.rs
- `stop_task` 停 Subagent 只按面板上的描述匹配，没有端到端用例（目前只测了后台 shell）
- 被 ccrpc 停掉的后台任务在主线忙时先记为 `stopped`，Claude Code 的结束通知要等当前工具结束才送达
- 全屏 TUI 下拍到的输入框、审批框、提问框、后台面板与默认 TUI 一致；计划审批只在默认 TUI 下实测

## GUI MVP 要的改动（只在本机用，不发布）

GUI 原是另起的 Tauri 应用 cc-gui，已并入 CoDeck 的 Connect 功能（仓库根 `docs/spec.md`「Connect」、ADR-0007）：对话内容来自 CoDeck 自己的索引，ccrpc 只提供状态与控制；CoDeck 每 Connect 一个 clay 里的 Claude Code 就连一个 `ccrpc attach`；TUI 同时在 Mac Client 里显示，认不出的界面在 TUI 里手动处理。

已做：`ls --json`（带标题）、`get_state` 的 `mode` / `model` / `context_tokens` 与 `session_info` 推送、共显、目录信任框作为 choice、`exit {close_pane, stop_tasks}`、`--log`、`progress` 事件、`get_messages` 的分页 / 增量缓存 / 后台线程 / `subagents`、`permission_request` 的 `options`、`choice` / `permission_request` 的 `layout_source`、`--log` 补足（版本、识别决策、整屏）、mock 端到端录 wire（`CCRPC_E2E_WIRE_DIR`）。

- 认不出的界面：v0 先经分析器归类，归不了的 GUI 只显示屏幕原文、在 TUI 里处理（不在 GUI 里按键）
- 斜杠命令、模型的候选列表

## 形态

- 常驻进程 + unix socket（一条连接管多个 Session，沿用同一套 JSONL 消息）；同一 Session 连多个 ccrpc（现在第二个 `attach` 报「已有 ccrpc 连着这个 Session」）。2026-11-05 评估后先不做：每个连接一个 ccrpc 时，一个 Session 约 15 MB，省资源不是理由；共享的收益只在多个调用方同看 / 同控一个 Session、同时连很多 Session 并快速切换（Session 列表的推送已由 `ls --json --watch` 做到）。代价是故障不再隔离、慢命令（`get_messages` 等最长 120 秒）互相拖住、升级要断开全部连接、脚本不能再直接管道驱动。真要做时依次考虑：只读的旁观连接（不占 hook socket、只收事件）；常驻路由 + 每个 Session 一个 ccrpc 工作进程（保留隔离与逐个升级）。
- 调用方 SDK（Rust、TypeScript / Python）与示例
- 分发：版本号、CHANGELOG、release 二进制

## 使用方（协议之上的子项目，各自单独设计）

- 主控 Claude Code 调度 Claude Code Worker（类 haa）：派 Task、取 Result、Ask / Approval 转给主控、插话、中止；事件以 monitor 推给主控；人可随时在 ToH 上接手
- ToH Client 的结构化操作：Session 状态（忙 / 闲 / 等审批）、审批卡片、发提示词、长任务完成推送（对应 ToH TODO 的「长任务完成通知推送」）
- 远程调用：经 ToH daemon 暴露，复用它的令牌、配对与 tailnet

## 机器忙时 mock e2e 偶发失败（2026-11-26 查过，作答与 Fork 已修）

复现：`cargo test -p ccrpc --test e2e mock_ -- --ignored --test-threads=6` 的同时在仓库根目录 `pnpm test:e2e --repeat-each=3`；失败用例的收发时间线看 cargo 打出的 stdout（或单条加 `--nocapture`）。

- 已修：**作答后状态不变**是 ccrpc 的问题，真实使用也会遇到。hook 放行后，提问框有一会儿会丢数字键与 `left` / `right`（见 probes.md），原先只靠固定睡 0.8s，键丢了也照样报 `request_end`，调用方的卡片没了、对话框还在等。现在每个键按下后等对话框变化，没变化时补一个 `down` 判断前一个键是不是被读到又丢了，确认丢了才重按；看到对话框离开才结束请求，超时请求留着。放行后不等直接按时这条路每次都会走到（`mock_model_ask_chat` 3/3 通过）。
- 已修：`fork` / `rewind` 按两次 `Esc` 后选择器没出现（满载下见过 2 次），现在 3 秒后仍是空闲的空输入框就再按，最多再按两次；选择器出现后的第一下 `↑` 没动同样再按一次。
- 已在用例里处理：启动时 ccrpc 报出的 `dialog(waiting_for: startup)`（进程文件出现前的 shell 回显，按 spec 会原地更新）不再当成错误；`ready` 时已经 idle 也算启动完；发 `prompt` / `exit` / `compact` 前先等 idle，按 `prompt_id` 认本轮的 `turn_end`。
- 和启动等待 0.5s / 1.5s 无关：两边交替对比，作答类全部成功。
- 已改：`start_session` 与 `settle_idle` 不再按时间推断，改为轮询 `get_state` 新增的 `input_ready`（输入框已画好）与 `settled`；测试配置写上 `hasSeenAutoDefaultNudge`，Opus 启动后不再晚弹 auto mode 提议（它曾挡住第一个 `prompt`）。
- 还没做（用例里的固定等待，2026-11-26 盘点过）：
  - `task_notification`：满载下前台的 `sleep 1` 比后台的 `sleep 2` 晚结束，通知并进同一轮（见过 4 次），后台改长或改成等标记文件。
  - `follows_the_background_session*`：`/model` 后固定看 5 秒有没有 `choice`，改成同时等 `choice` 或 `session_info`。
  - `harness.rs` 的 `wait_screen` 5 秒、`until_idle` 10 秒与 `mock_model.rs` 里 2–10 秒的期限在满载下偏紧（`follows_the_background_session_of_a_second_pane` 见过 1 次），期限只是上限，放宽不拖慢。
  - `follows_the_background_session`：空闲的整套里见过 1 次第一轮 `turn_end` 的 `usage` 全为 0（第二轮正常），像是 `turn_end` 先于 transcript 里带用量的条目被读到；单跑 5 次没复现。
  - `turn_end` 先于状态回到 idle，调用方紧接着发 `prompt` 会被拒（`not_idle`），机器忙时间隔更长；按 spec 是调用方要等 idle，要不要在 ccrpc 里给短宽限没定。

## 待确认

- 按键间隔现在常是 50ms（忙时 spinner 也算屏幕有反应），端到端都过；换慢机器或更长菜单时留意

## 已知限制

- `prompt` / `steer` 写入前照常清空输入框：人在 TUI 里写了没发的内容会被清掉（GUI 为主、TUI 里少写草稿，先不处理）
- 调成工作尺寸后不调回去：人在窄屏上看着时，那一端按 120×40 错排，直到它的 Client 抢回尺寸；已按过键的命令读不懂屏幕时只调尺寸、报错，要调用方再发一次
- 第一次查找 transcript 时偶发的文件系统错误会让已有 transcript 之后按实时读，重放历史 block
- 连续两次审批合并成同一段 waiting 的情形只做了逻辑处理（插件报来前一个的 PostToolUse 后重新认定），没有端到端用例
- 同时有几个后台 Subagent 在等审批、且工具输入里都没有可在审批框里比对的内容时，认不出屏幕上是哪个，改发 `dialog`
- Workflow Run 偶尔只读到 `journal.jsonl`：`workflow` 事件的 `name` 为 null、`phases` 为空直到结束（端到端偶发，`workflows/<runId>.json` 没被读到）
- 插件装在用户级：每次工具调用都会起一次 `ccrpc hook` 进程（不在 clay Pane 里立即退出），换了 ccrpc 可执行文件的位置要重新 `ccrpc plugin install`
- Subagent Run 文件变短（被替换）时从头重读，会重发其中的 block
- 原样重发的判定只认纯文字提示词：带图片的提示词重发也会报 `branch`；带图片的提示词也不参与 Branch 的分叉判定
- `get_messages` 的第一次要读完整个主文件（连接时就在后台开始），几十 MB 的 transcript 要等几秒
- 普通对话框等待（非审批）里连续出现两个菜单时，第二个 choice 会被吞掉（与 `dialog:` 公告的既有行为相同）
- 分析进行中（最长 60 秒）换成别的认不出的屏幕，仍显示原来那个 `analyzing: true` 的 `dialog`，要等分析结束才更新
- 审批类工具若以审批之外的 waitingFor 出现、又被学成标题不像审批的 choice，会盖住工具（未见过）
- 按数字键作答（编号审批框、choice、提问）时，若对话框恰在核对屏幕与按键之间被人关掉，数字会落到输入框；输入框为空且有反馈草稿卡片时，`2` 会把草稿发给 Anthropic（窗口很短，未见过）
- rewind：终端的回退列表按记录的条目类型决定列不列本地命令（2.1.329 实测：记成 `user` 条目的 `/model`、`/effort`、`!` 命令列，记成 `system` / `local_command` 条目的 `/context`、`/rename` 不列），ccrpc 照此排除；其余本地命令没有逐个实测
- rewind：文字相同的提示词靠选择器里的先后对应，再以确认页上的发送时间逐页核对；其中有插话（选择器是否列出未实测）时报 `ambiguous`，只能在终端里回退。在终端里直接回退过、还没发新提示词时，ccrpc 不知道终端的对话已变短，同样文字且发送时间落在同一档（如都是 2h ago）的提示词仍可能对应错

## 维护

- Claude Code 升级后重跑端到端；识别失败时用 `ccrpc debug screen` 拍新布局存进 fixture，更新 recognize.rs 与 probes.md 的版本
