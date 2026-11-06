# CoDeck 第一版规格

术语见 [CONTEXT.md](../CONTEXT.md)，关键取舍见 [docs/adr/](./adr/)。

## 定位

macOS 桌面应用，用来查看 `~/.claude` 下 Claude Code 的历史 Session，并能 Connect ToH clay 里运行中的 Session。查看的核心场景有两个：**找回某次对话**（搜索、定位）和**复盘某个 Session**（读得舒服）；统计是附加功能。除经 ccrpc Connect（见「Connect」一节，ADR-0007）外不对 Session 做任何写操作；数据目录始终只读（ADR-0002）。

## 平台与分发

- Tauri：Rust 后端负责解析、索引和监听，Web 前端负责全部 UI（ADR-0001）。
- 自用：本地构建，ad-hoc 签名，不做公证，不做自动更新；不开沙盒；macOS 14+，只出 Apple Silicon 版本。
- 仓库按可以开源的标准组织：有 README 和构建说明，不写死个人路径。

## 数据模型

- **Session** 以 session ID 为身份。符号链接导致的重复文件在扫描时按 realpath/inode 去重；同一个 ID 的多份真实拷贝按消息 uuid 取并集合并。
- **Project** 是一个 git 仓库的根目录：从目录的 realpath 向上找最近的 `.git`（不越过、也不取用户主目录）；`.git` 是指向 `<主仓库>/.git/worktrees/<名字>` 的文件时（git worktree，含 `.claude/worktrees/`），归入主仓库；不在任何仓库里的目录就是它自己。不支持手动合并 Project。（ADR-0006）
- **Session 与 Project 多对多**：
  - **启动 Project**：启动时 cwd 所在的 Project，每个 Session 恰有一个；中途 cd 不改变它；目录已不存在时，归入仍存在的上级目录所在的仓库（如已删除的 worktree），没有仓库时用原始 cwd，并标记"目录已不存在"；条目里都没有 cwd 时用 `~/.claude/projects/` 下的编码目录。
  - **改动的 Project**：Edit / Write / MultiEdit / NotebookEdit（主对话和 Subagent）写过的文件所在的 Project；被拒绝或失败的调用、数据目录（`~/.claude`）下的文件不算；不在任何仓库里、但位于启动 Project 下的文件算启动 Project，其他位置的文件不算；resume / Fork 复制进来的历史里的改动也算。按当前磁盘判断：仓库已删除的文件不再计入，用 Bash 做的修改（`sed -i`、生成代码等）不计入。
  - Session 属于它的启动 Project 和所有改动的 Project；在搜索里按 Project 查找、限定范围或筛选 Session 列表时，两者都算。
  - Project 只在搜索里出现（找 Session 的辅助）；Session 列表的行上、统计页都不显示 Project。
- **Subagent Run**：通过 `subagents/agent-<id>.meta.json` 里的 `toolUseId` 关联到主对话中的 Agent 调用，可以嵌套（`spawnDepth`）。
- **Workflow Run**：来自 `workflows/` 和 `journal.jsonl`。
- **Fork**：和其他 Session 共享消息 uuid 的 Session，独立显示，标注 Origin Session，继承来的历史默认折叠。
- **Continued Session**：Session 在 agents 页面转入后台时（← 进入 agents 页面，选中它后回车），原 Session 的 transcript 以 `continued-in` 条目结束、指向 Claude Code 新起的 Session，新 Session 复制原 Session 的条目（uuid、`message.id` 相同，没有 `forkedFrom`）后接着进行。新 Session 在索引里、且含有原 Session 主对话的全部用户输入（按 uuid；不比 assistant 消息，复制时会漏掉只有空 thinking 的回复）时，原 Session **被接替**：不进 Session 列表、Project 的 Session 计数、搜索、统计的 Session 数与排行、Fork 列表，不显示 Live 状态；按 id 打开它（Fork 的 Origin Session 链接、正打开着它时它被接替等）就转到新 Session（逐级跟随）。新 Session 是同一个对话的延续，不算 Fork：不标 Origin Session，历史不折叠。从原 Session Fork 出去的 Session 列在新 Session 的 Fork 列表里。两个 Session 互相 `continued-in` 时都不算被接替。复制不带上原 Session 目录里的 Subagent Run、Workflow Run 与 `tool-results/`：新 Session 自己没有的，就用原 Session 的（沿接替链往前找），关联、Subagent 面板、计数与 Cost 都算在新 Session 上，不再算在原 Session 上。新 Session 不在索引里或缺了原 Session 的用户输入（如原 Session 之后又接着对话）时，两个照常显示。
- **Branch / Main Line**：消息按 parentUuid 构成一棵树。Main Line 是走到最新叶子的那条路径（优先用 `last-prompt.leafUuid`，否则取时间戳最新的叶子）。
- **compact**：compact 边界会切断 parentUuid 链；边界通过 `logicalParentUuid` 接回压缩前的历史，但只接受文件中位置更早的条目（否则会成环），找不到时接到文件中前一条消息，因此 Main Line 跨越 compact 保持连续。
- **Fork 判定**：条目带 `forkedFrom` 的即为 Fork，继承部分以此标记；没有该字段时，以和更早的、未被接替的 Session 共享首条消息作为兜底。
- **Live Session**：`~/.claude/sessions/<pid>.json` 存在、进程存活且 `procStart` 一致；`status` 区分 busy、waiting 和 idle。不看 transcript 的修改时间：Claude Code 退出时删掉进程文件，刚写过的 Session 也可能已经结束。
- 标题优先级：`custom-title` > `ai-title` > 第一条用户输入。
- **用户输入的判定**：只有用户亲手输入的内容、斜杠命令和 `!` shell 命令算作"你"的消息；系统注入、任务通知、命令输出、compact 摘要和 Teammate Message 都不算，不参与消息数、分支、标题和"你"角色的搜索。Claude 工作期间输入、排队到下一轮才送达的内容，文件里只记为 `queued_command` 附件（没有对应的 `user` 条目），按同样的规则当作用户消息处理：亲手输入的算"你"的消息，排队的任务通知、Teammate 消息按各自类型显示。新版 Claude Code 把 `/rename`、`/model` 这类本地执行的斜杠命令及其输出记成 `system`（`local_command`）条目，旧版记成 `user` 条目，两种按同样的规则处理：命令算"你"的消息，输出按命令输出显示，空输出隐藏。长段粘贴被 Claude Code 包在 `<pasted_content id="xxxx">` 标签里时（账号级开关），显示、标题和搜索都用去掉标签后的原文。
- **计数口径**：消息数 = 用户输入 + assistant 消息（按 `message.id` 合并拆分的片段），只计主对话；token 按 `message.id` 只计一次，每个字段取各片段的最大值；Session 的 token 合计包含其 Subagent。同一条 assistant 消息出现在多个 Session 中时（resume / Fork 会把之前的对话复制进新 Session 的文件），token 和 Cost 只计入最早创建的那个 Session，统计也只计一次（与 ccusage 一致）；Session 被删除或被接替后，归属转给下一个仍包含它的 Session（被接替的 Session 排在最后）。消息数不受影响；统计不计被接替的 Session 的消息数，它仍计费的 token 与 Cost（复制时漏掉的回复）照常计入总数与按天、按模型的图表，但不出现在 Session 排行和下钻出的 Session 列表里。
- **Cost**：参照 ccusage 的算法，按 LiteLLM 价格表逐条计算 assistant 消息：输入、输出、缓存读取、缓存写入各按单价计；缓存写入区分 5 分钟和 1 小时（1 小时按价格表中的单价，没有时按输入单价的 2 倍）；模型有 200K 以上单价时，各项超过 200K 的部分按该单价计；fast 模式乘以模型的 fast 倍率；条目自带 `costUSD`（旧版 Claude Code）时直接采用。模型名去掉 `[1m]`、`@地区`、日期后缀再匹配；匹配不到的消息不计入金额并单独计数，界面上提示"无价格"。
- **价格表**：内置一份 LiteLLM 中 `claude-*` 模型的快照；启动时以及之后每 10 分钟检查一次，缓存超过一天就在后台重新下载 LiteLLM 价格表（ADR-0005），存到 Caches 下的 `pricing.json`，价格有变化时重新计算全部 Cost。下载失败沿用缓存或内置快照，不提示。
- 数据根目录：默认 `~/.claude`，识别 `CLAUDE_CONFIG_DIR`，也可以在设置里修改；只支持一个数据根目录。不读取 `~/.claude/transcripts/`。

## 索引

- 可以丢弃的镜像缓存，存放在 `~/Library/Caches/<bundle-id>/`；源文件删除后对应 Session 随之消失；schema 变化时直接重建（ADR-0003）。
- 增量方式：按文件记录已读到的字节偏移；文件变小或 inode 变化时整个文件重新解析。用 FSEvents 监听变化。
- 首次启动先扫描元数据，几秒内列表就能用；全文索引在后台分批构建，Session 列表底部显示"正在建立全文索引 · x / y 个文件"；文件变更会插队处理，之后索引继续。实测 3 GB 数据约 2–3 分钟建完，索引约 760 MB。
- 宽容解析：以行为单位，失败的行跳过并计数；未知的条目类型、内容块、工具都用通用方式显示，不丢内容；所有字段按可选处理。

## 搜索

- 入口：⌘K 或红绿灯右侧的搜索图标打开搜索浮层，再按 ⌘K 关闭；不用双击 Shift，以免和输入法切换冲突。不设过滤器条。
- 语义：字面子串匹配，支持多词 AND、`"短语"`、`-排除词`；Session、Project 和对话内容用同一套规则，只有排除词时不匹配。
- 结果固定分三段，后到的结果只追加在下方，已选中的项不跳：
  - **Project**（最多 3 个）：名称或路径命中。↵ 用它筛选 Session 列表（列表收起时自动展开）；⌘↵ 把它设为搜索范围，显示为输入框前的标签，三段都只在其中，输入为空时 ⌫ 移除。
  - **Session**（最多 8 个）：标题、Project 名、git 分支命中，在前端即时匹配，不等防抖；命中 Project 名或分支的词越多越靠前，其余按最近活跃。空输入时列出最近的 Session（最多 20 个）。
  - **对话内容**：全文索引，200 ms 防抖后出现，按 Session 分组，每组最多 5 处。
- ↑↓ 在各段之间连续移动，⇥ / ⇧⇥ 跳到下一段 / 上一段的第一项；打开的 Session 不在当前 Project 筛选里时清除筛选。关闭后保留输入和范围，再打开时全选输入框。
- 默认范围：用户输入、Claude 回复（含存成 thinking 块的 narration）、工具输入，用 SQLite FTS5 trigram 索引；AskUserQuestion 的回答和备注按"你"的角色索引，打开命中定位到对应的问答；少于 3 个字符的词（如两个汉字）改用子串匹配，这种情况下只对 ASCII 字母忽略大小写。
- 工具输出不建索引，也不搜索（ADR-0004）。
- 输入法组字期间不过滤也不发起搜索，确认上屏后才搜索（⌘F 同理）；组字时按 Esc 只取消候选词，不关闭搜索或查找。
- 不建索引：attachment、hook 输出、系统注入、图片、元数据。
- 对话内容按 Session 分组，按时间倒序；全文索引未建完时提示进度，Session 与 Project 的匹配不受影响；命中位置在 Subagent 里时标注其 agentType，在已回退的 Branch 里时标注"已回退的分支"；打开命中会自动切换分支、打开 Subagent 面板并定位高亮。
- 不做语义搜索，但保留以后扩展的可能。

## 界面

- 两栏布局：Session 列表 | 会话区（选中 Session 的整栏：头部、对话记录，Connected Session 的底部另有悬浮的输入区）；红绿灯右侧是 Session 列表开关和搜索图标（悬停提示 ⌘K），全屏时没有红绿灯，图标移到最左边；对话头部不另设搜索入口。没有底部状态栏：Session 列表底部一行是索引状态（已索引 N 个 Session / 进度 / 失败）、统计按钮和设置按钮（⌘,）；列表收起时统计和设置跟在左上角的列表开关、搜索图标后面。
- 时长分两档：概览（Session 头部的时长、Connect 的上一轮用时）只到分钟，如「1 小时 12 分」「5 分」「30 秒」；过程细节（每轮「已处理」、Subagent、工具调用的用时）精确到秒，如「1 分 04 秒」，不足 1 秒写「320 毫秒」。
- 统计以大弹窗盖在窗口上显示，点弹窗外、Esc 或右上角关闭按钮关闭；从统计下钻或打开 Session 时自动关闭；打开搜索时也会关闭统计。
- Session 列表顶部：红绿灯那一行只放上述两个图标；下一行是「全部 Session」与数量，右侧是排序（如「按时间 ↓」，菜单切换排序字段和正序/倒序）。从搜索里按 Project 筛选时，标题位置显示 Project 名，× 清除。
- 打开 Subagent 面板不改变 Session 列表的收起状态与左上角的工具栏，面板占用对话区的宽度。
- Session 列表可收起（⌃⌘L），收起状态在重启后保留；Session 列表收起时在搜索里选择 Project 会自动展开它。
- 各栏头部、对话头部和 Session 列表底部一行可以按住拖动窗口（按钮、输入框除外）。
- 窗口整体不随触摸板回弹，只有各栏内容区自身滚动。
- 滚动条只在滚动时或指针停在滚动条上时显示，不受系统"显示滚动条"设置影响。对话记录（含 Subagent 面板）的滚动条浮在内容右缘，不占宽度。
- 轮次大纲浮在对话列左侧的留白里，不占宽度：每轮一道刻度，上下居中，放不下时先压缩刻度间距（最小 1px），再放不下就往下延伸（不盖住标题栏），当前轮为蓝色，悬停的刻度变长并在右侧显示该轮输入。
- Session 列表每行显示：标题、相对时间、消息数、token 数、Cost（时间、消息数、token、Cost 都可以排序）、Live 标记、git 分支；从统计下钻时，列表顶部显示可清除的筛选标签。
- Session 头部两行：标题行是标题、「Connect」（可 Connect 时）或「Resume」（已结束、可 Resume 时）和「···」菜单（显示系统消息、复制 Resume 命令、复制 Session ID、在 Finder 中显示、Connected Session 的复制排查信息）；元信息行是状态（一个词：进程在跑时圆点加「工作中 / 等待作答 / 空闲」，已连接时取 ccrpc 的状态，否则取 Live 状态；已结束写「已结束 · 3 分钟前」）、Project · 分支、Cost（含 Subagent；悬停显示按模型的明细，有无价格的消息时后带"+"）、时长、消息数，以及 ⓘ 悬停卡片（输出、输入、缓存读取 token，工具调用次数、Subagent 数、模型；各占一行，数字右对齐）。列表行 Live 点的悬停提示用同样的词。
- 对话渲染（参考 Codex）：
  - 单栏居中，最宽默认 720px：拖动列的左右边缘调节（两侧对称，480–1600px，记住上次的宽度），双击边缘恢复 720px，边缘获得焦点时 ← / → 每次 20px；悬浮的输入区与列同宽。你的消息是右侧气泡，照输入原样显示，只把 `` ` `` 包着的行内代码与 ```` ``` ```` 围起的代码块显示为代码、反引号不显示（行内代码不跨行；围栏写在行首，不闭合时到消息末尾；代码块按语言高亮）；下方是时间、复制（悬停时出现）和 Branch 切换器；Claude 的回复没有「Claude · 时间」抬头，只在 Branch 起点显示切换器、API 错误时显示「API 错误 · 时间」。
  - 按轮折叠（参考 Codex）：最后一次工具调用、思考、Subagent 或 Workflow 之后的文字是这一轮的「回答」，最后一段下面是复制（复制整段回答）和时间。回答之前的过程（中间说明、思考、工具调用、Subagent、Workflow、提问）只要含有工作，就折叠成一行「已处理 1 分 50 秒 · 4 段说明、5 次工具调用」（用时从你的消息到回答开始；没有回答时到最后一条过程），点开后按原顺序排开；没有工作的轮次不折叠。还在进行、尚无回答的最后一轮默认展开。展开状态随 Session 记住，直到切换 Session；搜索、⌘F 或跳转命中折叠里的内容时自动展开那一轮。
  - 每个工具调用是一行字（图标、动作如「运行 / 读取 / 编辑 / 写入 / 搜索」、对象，失败或在等的才在右侧标出，另有用时），点开在下方的框里看输出、diff。展开的一轮过程里每次工具调用各占一行，不再合并（过程已按轮折叠）；没有按轮折叠的地方（Subagent 面板）连续的工具调用合并成一组，默认收起，组头一行写做了什么（「读取 1 个文件、编辑 2 个文件、运行 1 条命令」，编辑按路径去重，其他工具写「等 N 次工具调用」）、对象（文件名、命令、搜索词）和失败数。
  - 专用渲染器：Bash（终端风格，支持 ANSI；展开后先写完整命令再写输出；后台命令（`run_in_background`）在行尾标「后台」，输出处写它的任务 ID 与「跳到结束通知」）、Edit（diff）、Write、Read（默认不展开内容）、Task 清单；其他工具用通用 JSON 视图，输入里有网址（如 Artifact 的 `url`）时可点击、在浏览器中打开：摘要就是网址时摘要本身是链接，摘要是别的字段（如 `path`）时网址跟在它后面。
  - AskUserQuestion 不并入工具组、不折叠：一张卡片列出全部问题和选项，就地标出所选项（含"其他"里输入的内容）和备注，回答不再另外重复显示；卡片头部注明"你 · 时间 已回答"、等待回答或未回答（被拒绝或调用失败）；用户选择先聊聊（Chat about this）时文件里记为拒绝，卡片按正常样式显示"你 · 时间 选择先聊聊"，并标出此前已答的问题。回答不计入消息数，不开启新的轮次。
  - 思考块：有内容时折叠成一行「💡 思考 N 行」（首尾空行不显示、不计），展开后是浅底块；空的隐藏。Claude Code 把写给你看的过程说明（narration）也存成 thinking 块（签名里的类型为 `narration`，真正的思考存为空文本），终端按正文显示，这里同样当作正文，也进全文索引。
  - 字号：Claude 的正文（回答与过程里的说明）与你的消息都是 14px，和输入框一致。
  - attachment 和系统注入默认隐藏，可以用开关显示；调用不在当前路径上的工具结果同样隐藏，显示时标为「工具结果」。
  - compact 边界显示为分隔线；`tool-results/*.txt` 按需加载（是符号链接时不读取，按缺失处理）；图片显示缩略图（斜杠命令里粘贴的图片记在命令之后的元数据条目里，显示在命令上）。
  - Branch 用 `‹ i/n ›` 切换；如果被放弃的分支只是原样重发且后面没有回复，不显示切换器。
  - 任务通知、Teammate Message 是没有边框的灰色细行（任务通知居中）。
  - 斜杠命令像终端一样高亮：命令名用等宽强调色，参数保持正文字体。`!` shell 命令（文件里是 `<bash-input>`）同样处理，显示为 `!git status`，标题也是这个样子；它的输出（`<bash-stdout>` / `<bash-stderr>`）按命令输出显示。
  - Teammate Message 不显示为用户气泡，一条里的几段（`<teammate-message>`）按种类显示：报告（普通文字，及认不出类型的 JSON）是可折叠的"Teammate · <id>"卡片，相邻的合成一张；一轮结束（`idle_notification`，claude 2.1.251 起带 `result`，即它这一轮最后的回答）是一张「<名字> 完成」卡片，正文按 Markdown 显示，过长时先截住、可展开全文；没有 `result` 的写「空闲」，`idleReason` 为 `failed` 的写「失败」与 `failureReason`；退出（它的 `shutdown_approved` 与 `system` 的 `teammate_terminated`，同一条里的合成一行）写「已退出」；分配任务（`task_assignment`）写「<分配者> 分配任务 · #编号 主题」。这四种都是居中的一行。
  - 其他 Agent 经 SendMessage 发来的消息（`origin.kind` 为 `peer`）：另一个 claude 会话的 `<cross-session-message>`、本 Session 的 Subagent 或 Teammate 的 `<agent-message>`；插件的消息（`The <名字> plugin sent a message:`）同样处理。空闲时送达的被 claude 记为 meta、忙时送达的记为排队命令，都显示为可折叠的「会话 / Agent / 插件 · <发送方>」卡片，去掉外层标签与套话，不算"你"的消息。
  - 当前 Session 读取失败时显示一行错误；源文件被删除时回到空状态。
- Subagent：主对话中显示为卡片（任务 + 结果 + 状态：运行中 / 已完成 / 失败 / 未收到完成通知），teammate（带 `name` 派出、加入 agent team 的，工具结果是 `teammate_spawned`）的卡片另标「Teammate · <名字>」与它的颜色，状态与结果取自它发回主线的消息：结果是它最近一次的报告或一轮结束的 `result`（派出时的工具结果不当作结果），状态为运行中 / 空闲 / 已退出（主线发给它 SendMessage 时转为运行中；Session 不再活跃、又没见到退出时不写状态）；右侧面板的头部也写状态；后台 agent 以结束通知为准，收到之前不算结束（Session 不再活跃时写「未收到完成通知」），结果取通知里的 `<result>`（没有时取 summary；resume 后再次结束会再通知，状态与结果取最后一次，再次运行期间仍显示上一次的结束），启动时的工具结果不当作结果；"查看完整过程"在右侧面板打开，嵌套时显示面包屑；Subagent Run 里派出方后来发来的消息（SendMessage，Claude Code 记为元数据，`origin.kind` 为 `coordinator`）去掉外层套话后显示为「主对话发来的消息」（嵌套时为「上级 Subagent 发来的消息」）卡片，开始新的一轮；后台 agent 的卡片、后台命令的工具调用和结束通知互相链接，被 SendMessage 续接过的 agent 再结束时通知指向那次 SendMessage，通知行的「查看派出位置」仍到最初的 Agent 调用，另有「查看续接位置」；后台任务结束的通知（`<task-notification>`）一行显示，按 summary 的前缀标明是后台 agent、后台命令、Monitor 还是 Workflow，写出描述（没有描述的后台命令写命令本身，过长截断）、状态，以及非 0 的退出码或失败原因，认不出的写「后台任务」加原文；summary 里的 XML 转义（`&gt;` 等）还原后显示；Workflow 显示为一张总卡片。
- 阅读便利：⌘F（会自动展开折叠的内容，包括工具组内的思考块和截断的工具输出）、`j`/`k` 按轮次跳转（对话框打开时不响应）、轮次大纲、复制消息/代码块/Bash 命令、复制 Session ID 和 `claude --resume <id>`、在 Finder 中显示。
- 不做：导出、URL scheme、书签和笔记、汇率换算、Connect 以外任何写 Session 的操作。
  - 导出为 HTML 评估过，暂不做：对话列表是虚拟列表，导出要另行整页渲染；按需加载的工具输出、图片要一次性取全；Shiki 高亮是异步的，要等全部完成；折叠需改成 `<details>`；保存位置需引入对话框插件。只导出 Main Line、工具保持摘要与预览的首版约 1–2 天，带完整工具输出和 Subagent 过程会更多。

## 统计面板

时间范围；概览卡片（Session 数、消息数、Token、Cost、活跃天数）；每日 token 柱状图；活跃时段；Session 排行（按 Cost，最多 10 个，在右侧跨两行）。不统计工具调用和 Subagent。页面背景铺满窗口高度，图表保持固定高度，不随窗口拉高；头部（标题、时间范围）固定，只有下方内容滚动。不按 Project 过滤或排行。

- Token 卡片占两格：输入与输出两个数分居左右两栏（位置不随数值长短变化），右上角缓存命中率，下方一条构成条和图例（缓存读取 / 缓存写入 / 未缓存输入，悬停说明各自含义与计价倍率）。输入 = 未缓存输入 + 缓存写入 + 缓存读取；缓存命中率 = 缓存读取 ÷ 输入。
- 每日 token 可切换「全部 / 输入 / 输出」：全部是每天输入、输出两根柱子；输入、输出按模型堆叠，图例可下钻。
- Session 日均数不足 10 时保留一位小数。
- 活跃时段是时间直方图：每根柱子是一段时间内主对话的消息数，柱宽从 1h、3h、6h、12h、1d、1w、1mo 中自动选最细、且柱子不超过 170 根的一档（7 天 → 1h，30 天 → 6h，90 天 → 1d，「全部」按从最早一条消息到现在的跨度）；按本机时区对齐（整 n 小时、0 点、周一、1 号），没有消息的柱子为 0；夏令时跳过的小时没有柱子，回拨重复的小时并入同一根柱子；按月仍超过 170 根时只显示最近 170 个月。卡片右上角标柱宽，没有模式切换；坐标用英文和数字，悬停提示用中文。点击柱子把 Session 列表筛到这段时间。

- 时间范围：标题右侧一个下拉，分组列出 周期（本周期、上一周期）/ 月份（本月、上月）/ 最近（最近 7 天、30 天、90 天）/ 全部，默认本周期。「周期」从每周重置时间算起 7 天；重置时间（星期几 + 时刻，本机时区，默认周一 0:00）在「设置 › 通用 › 统计周期」里设置，重启后保留。「月」是自然月。选周期或月份时，下拉右边的 ‹ › 按同一单位前后翻（往后最多到当前周期 / 当月），下拉显示「本周期 / 上一周期 / M月d日起的周期」「本月 / 上月 / yyyy 年 M 月」；过去的周期、月份统计到其结束时刻，当前的统计到现在。最近 N 天与全部不翻页（‹ › 置灰）。同一行显示所选周期的起止时间或月份，当前周期 / 当月再加剩余时间。活跃天数的分母是所选范围的天数。切换时间范围时页面布局不变。
- 口径：消息数和活跃时段只计主对话；token 和 Cost（概览、每日、Session 排行）包含 Subagent，跨 Session 重复的消息只计一次；排行里的金额只算时间范围内的消息。日期按本机时区划分。
- 下钻：点击每日 token 的柱子、活跃时段的柱子、模型图例会筛选 Session 列表，并带上当前的时间范围；点击 Session 排行打开该 Session。

## 诊断

在设置（⌘,）的"格式兼容性"页：解析失败的行（含文件与首个错误）、未知的条目类型 / 内容块 / 系统子类型 / 工具名及出现次数和涉及版本、认不出的消息（按"你"的消息处理却带结构标记的 user 条目：开头是认不出的标签，或 `origin.kind` 不是已知的几种，按标记汇总；新版 claude 加了消息类型时在这里出现）、各 Claude Code 版本汇总（标题下写共几个版本、从最低到最高版本；表里只列有失败行或未知项的版本，都没有时不显示表）、文件内重复 uuid 数、未关联的 Subagent 数；可复制报告、重建索引。

## 设置

数据根目录按"设置中的覆盖 → `CLAUDE_CONFIG_DIR` → `~/.claude`"确定；覆盖值保存在 `~/Library/Application Support/<bundle-id>/settings.json`，切换后清空当前视图并重新扫描。

- **界面主题**（设置 › 通用）：自动（默认，跟随系统外观，系统切换时跟着变）/ 浅色 / 暗色；配色由 `<html>` 的 `data-theme` 切换，窗口本身（标题栏、菜单、选目录对话框）经 Tauri 的窗口主题一起切换；选择存在 localStorage，启动时在首次绘制前套用，不闪一下另一种配色。
- **自动 Connect**（设置 › 通用，默认开启）：打开可以 Connect 的 Session（在 `ccrpc ls` 里、插件已报到、没有连接、它的 Holder 没被别的连接占着）并停留 300ms 后自动 Connect（用 ↑↓ 经过的 Session 不连）；每次打开只试一次：手动 Connect、Disconnect、连接失败或连接已结束后都不再自动连，换到别的 Session 再回来才会再试。存在 localStorage。

## Connect

经 ccrpc（[crates/ccrpc](../crates/ccrpc)，协议见其 `docs/spec.md` §5）控制 ToH clay 里运行、装了 ccrpc 插件的 Session（ADR-0007）。**对话内容仍来自本应用的索引与实时追随，ccrpc 只提供状态与控制**：不调用 `get_messages`，不用 ccrpc 的内容块、`branch`、`workflow` 事件（内容块只用来知道某次工具调用已执行、对应的请求结束）。Rust 后端（`src-tauri/src/ccrpc`）只管 ccrpc 子进程、转发 JSONL、写日志；协议解析与状态在前端（`src/features/connect`）。术语另见 ccrpc 的 [CONTEXT.md](../crates/ccrpc/CONTEXT.md)（受控 Session、请求、选择、轮）。

- **环境**：第一次用到 ccrpc 时读一次登录 shell 的环境（`<shell> -l -c env`，`<shell>` 是账户记录里的登录 shell，不是 CoDeck 继承来的 `$SHELL`），作为所有 ccrpc 子进程的环境，其中 `SHELL` 设为这个 shell：`toh new` 用它起 Pane，CoDeck 不论从 Dock 还是终端启动，Session 都跑在账户的登录 shell 里，带着它的配置设的环境变量。不在启动时读，以免拖慢启动。ccrpc 依次取环境变量 `CODECK_CCRPC`、`~/.local/bin/ccrpc`、PATH 里的 `ccrpc`。
- **可 Connect 的 Session**：窗口可见、且至少有一个 Live Session 或连接时，后台常驻一个 `ccrpc ls --json --watch`：开始时给一次全表，之后 Pane 来去、隐藏 / 显示、claude 的状态与报到有变化时再给（由文件与进程事件唤醒，不定时查询）；条件不满足时停掉它（没有 Live Session 也没有连接时丢掉上次的结果），条件恢复时重新开始；停掉之后才到的结果丢弃。Holder 的 `session_id` 就是 Session ID。`ccrpc ls` 启动不了、自己退出了或读取 ccrpc 信息失败时停掉，错误显示在「设置 › ccrpc」，可重试。
- **标题栏**：选中的 Session 在 `ccrpc ls` 里且插件已报到时显示「Connect」（`ccrpc attach`）；插件未报到时按钮禁用，悬停提示「ccrpc 插件未加载，重启 claude 后可 Connect」；不在列表里（不在 clay 里，或 ccrpc 不可用）时不显示。已连接时同一位置换成「Disconnect」（关闭 ccrpc 的 stdin，Session 照常运行；ccrpc 5 秒内没有退出，或 stdin 被卡住的写入占着时，向它发 SIGTERM，ccrpc 做完当前命令后退出，150 秒仍未退出才强制结束；clay 里的 Session 都不受影响）与「关闭」（ccrpc `exit {close_pane, stop_tasks}`：结束 claude 与它的 Pane，再 Disconnect；忙时先 `abort`；忙或还有运行中的后台任务时先弹确认，写明将中止本轮、停掉几个后台任务；空闲或忙时可用），头部不另写「已连接」，状态词取 ccrpc 的状态；「···」菜单多一项「复制排查信息」（holder_id、session_id、cwd 与这个连接的日志目录）。连接保持到 Disconnect、Session 或 ccrpc 退出、应用退出。Session 结束后（`session_exit`；ccrpc 仍在运行，但不再连着任何 Session）同一位置重新显示「Connect」（Pane 里又有 claude 在跑时，如在终端里 resume 了）或「Resume」，连接前先关掉这个 Session 或 Pane 上已结束的连接（等它的 ccrpc 退出）。
- **Resume**：已结束（不是 Live Session）、不在 `ccrpc ls` 里的 Session，标题栏在「Connect」的位置显示「Resume」：在新的 clay Pane 里从它的启动目录（transcript 里第一次出现的 `cwd`）运行 `ccrpc new --cwd <目录> -- [启动参数] --resume <id>`，然后像 Connect 一样连着它；Session ID 不变，不产生新 Session。总是从 Main Line 末尾继续（`claude --resume` 的行为），开始时视图切回 Main Line；悬停提示写明这一点。还在运行的 Session（包括在 clay 外的终端里）不显示，点下时后端也重读一次进程文件，仍在运行就拒绝（「Resume 失败：Session 仍在运行」），以免两个 claude 写同一个 transcript；启动目录不存在、不知道启动目录、`ccrpc ls` 或 ccrpc 信息读取失败时按钮禁用，悬停提示原因。进行中按钮写「Resume 中…」（不出现「新 Session」占位行）；claude 在得知 Session ID 前退出（如 `No conversation found with session ID`），或 30 秒内没有得知时（有等你作答的卡片时不计，如目录信任框），算作失败：读一次 Pane 的屏幕，以 claude 的报错行（没有时取光标所在行之上的最后一行）为原因（超时写「30 秒内没有启动（<那一行>）」），标题栏写「Resume 失败：<原因>」，再关掉这个 Pane（`exit {close_pane}`）并断开；claude 还停在自己的界面上、Pane 关不掉时，原因后面写明。失败处理完之前（以及 Resume 进行中）不能再点 Resume。claude 自己的恢复确认界面（「This session is … old and … tokens」，长时间没动的大 Session 才有）按认不出的界面处理，在 Pane 里作答。
- **输入区**：悬浮在对话记录底部（参考 Codex：对话从它下面滚过，上沿渐隐；对话末尾留出它的高度），与对话记录同宽，只在当前 Session 已连接时出现。提示都是输入框上方的圆角卡片（图标、标题、一句说明，右侧动作与关闭）。自上而下：ccrpc 意外退出（退出码与 stderr 最后一行 +「重连」，不自动重连）、Session 已结束（可 Resume 时卡片上带「Resume」；这两种情况只显示这一张，没有请求卡片和输入框）、通知（学会新界面、界面识别失败，可关）、错误（标题是失败的操作，说明是原因，可关）、请求卡片、Claude Code 自己的排队（ccrpc 的 `queue`：插话和人在终端里忙时输入的，与 TUI 一致；「立即发送」= `send_now`，即终端里的 ctrl+enter，中断本轮立即发出）、输入框。
- **对话记录末尾**：已连接时，对话最后一条消息下面一行，忙时是 Claude Code 的进度（`progress`，如 `Thinking… (3s · ↓ 120 tokens)`；灰色，一道高光从左往右扫过，系统开了「减少动态效果」时静止），否则是上一轮的用时、token、Cost、错误、已中止；停在底部时随它（以及输入区变高）保持在底部。
- **发出的提示词**：从输入框发出消息（提示词、插话、斜杠命令）时，对话记录像聊天应用一样滚到底并保持在底部。空闲时按 Enter 发 `prompt`，输入框立即清空，对话记录末尾（上一轮的结束行之后、进度行之前）出现一个占位气泡：与提示词同样的气泡，里面只有三个跳动的点（聊天应用里「正在输入」的样子），不显示提示词文字（以索引里的为准）。ccrpc 回应后立即让后台同步这个 Session（`sync_session`，不等文件监听的防抖）；索引里出现这条提示词（ccrpc 回的 `uuid`）时占位气泡换成它，它从占位处浅入放大到原样（220ms），时间与复制随后淡入；gui.log 记一行从发出到出现的毫秒数。发送失败时占位去掉，文字回到输入框（之后又输入的接在后面），错误卡片写「发送失败：<原因>」。忙时插话、走 `slash` 的斜杠命令、ccrpc 报进了队列的不放占位；20 秒内没等到就去掉。系统开了「减少动态效果」时不播放动画。
- **输入框**：没有单独的提示行，占位文字按状态说明 Enter 做什么（空闲：发消息；忙：插话，⌘Enter 中断本轮、立即发送，Esc 中止；等待作答：先回答请求；连接中）。底部一行（参考 Codex）：左侧是权限模式（可点，`set_mode`；空闲或忙时可切换；自动模式用橙色）、粘贴的图片数、后台任务（只在有运行中的任务时出现，按种类写运行中的个数，如「1 Shell · 2 Subagent · 1 Monitor」，种类有 Shell、Subagent、Monitor、Workflow；点开按种类分组列出运行中的，Shell 与 Monitor 另写命令，可停止，已结束的点「显示已结束的」才列出；Subagent 点名称在右侧面板打开它的完整过程（teammate 在 ccrpc 里另有任务 id，按描述对上它的 Subagent Run），Shell、Monitor、Workflow 点名称跳到主对话里启动它的工具调用，调用不在当前显示的对话里时不可点）；右侧是上下文用量（圆环 + token 数，窗口按 200K 计，`[1m]` 模型按 1M）、模型选择器、最右是发送——忙或等待作答且输入框为空时变成「中止」（`abort`，同 Esc）。出错时输入框上方显示错误卡片。
- **模型与思考等级**（照 Claude Code Desktop）：底栏右侧按钮写模型（`claude-opus-4-7` 显示为「Opus 4.7」，`[1m]` 加「(1M)」）和浅灰的思考等级（xhigh 写「Extra」，模型没有思考等级时不写）。点开：模型（默认、Opus、Fable、Sonnet、Haiku，当前所属的打勾）→ `set_model` 别名；「Effort ›」二级菜单（一句说明，Low / Medium / High / Extra / Max）→ `set_effort`，只在模型有思考等级时出现；「其他模型 ›」可输入模型名。只在空闲时可点（Claude Code 只在空闲时接受 `/model`、`/effort`）。与终端里直接执行 `/model X`、`/effort X` 一样，也会成为新 Session 的默认；对话中途第一次切换时 Claude Code 先问「历史要重新读一遍」，显示为选择卡片，由你作答。切换后按钮立即更新（ccrpc 从命令输出与输入框右上的思考等级标记读出）。斜杠命令补全列表底部写按键（↑↓ 选择 · Tab 补全 · Esc 收起）。

  | 状态 | Enter | 其他 |
  |---|---|---|
  | idle | `prompt`（见下方「发出的提示词」） | |
  | busy | `steer`（与终端里忙时按 Enter 一样：进 Claude Code 的队列，TUI 里可见，当前工具调用结束后送达；没有工具调用可夹带时排到本轮之后） | ⌘Enter：`steer` 后 `send_now`（输入框为空时只 `send_now`），同终端里的 ctrl+enter |
  | waiting | 不发送，焦点移到第一张请求卡片 | |
  | busy / waiting | | Esc `abort`（焦点在输入区时） |

  - 粘贴的图片存到 `~/Library/Caches/dev.joy.codeck/images/`，输入框上方以 `data:` 地址预览，路径作为 `images` 传给 `prompt`，忙时一样传给 `steer`（队列里这条以 `[Image #n]` 开头，同终端）。
  - 斜杠命令：idle 时内置命令（以及补全清单里没有、形如命令名的名字）走 `slash`——本地命令和打开面板的命令在 transcript 里不留可等的记录；自定义命令、skill、`/Users/…` 这类路径、多行或带图片的，以及取不到补全清单时，走 `prompt`。忙时照常 `steer`，但会打开界面的内置命令不发送，提示等空闲时再发（从队列里打开的界面会一直挡着本轮，要到终端里关）。
  - `!` 命令：idle 时以 `!` 开头的内容（一行、不带图片）照终端的样子在 shell 模式里运行（ccrpc `prompt`），输出交给模型、自成一轮；忙时不能发（ccrpc 报「这里不能发 ! 命令」）。
  - 补全：输入框只有 `/名字`（还没有空格、换行）时，上方列出匹配的命令（名字完全相同 > 前缀 > 命名空间 `:` 之后的前缀 > 包含）；只输入 `/` 时列出全部，列表可滚动，显示描述、来源（内置 / 用户 / 项目 / 插件），会在终端里打开界面的内置命令标「打开界面」；↑↓ 选择，Tab 补全，Enter 在选中项与输入不同时补全、相同时发送，Esc 只收起列表。清单来自内置命令表、数据根下的 `commands/`、`skills/`，Session cwd 下的 `.claude/commands`、`.claude/skills`，以及用户和项目设置里启用的插件的 commands / skills（插件的以 `插件名:` 为前缀）；只读，同名时前者优先。
  - 高亮：输入框开头是补全清单里的命令（`/名字`）时，这段以强调色显示；句中的 `/名字`、清单里没有的名字不高亮。
  - `/add-dir` 补全目录：输入 `/add-dir ` 之后，上方列出所写路径那一层里名字以已写部分开头的目录（不分大小写，按名字排序，至多 50 个），每项写作已写的路径加目录名与 `/`；相对路径以 Session 的 cwd 为起点，`../` 上一层，`~/` 为主目录，绝对路径照写；隐藏目录要在名字以 `.` 开头时才列，写到 `.` 或 `..` 时另列 `../`。↑↓ 选择，Tab 补全（补全后接着列出其中的目录），Enter 在用 ↑↓ 选过时补全、否则发送，Esc 只收起列表。只列目录，不读写其中的文件。
  - `@` 补全文件：光标所在的词以 `@` 开头（`@` 在开头或空白之后；`me@host` 这类不算）时，同样列出所写路径那一层的文件与目录（目录以 `/` 结尾，名字带空白的不列），规则同 `/add-dir`；补全只替换这个词，补全文件后接一个空格、列表收起，补全目录接着列出其中的内容。
  - 共显时写入会清空 TUI 输入框里没发的内容（ccrpc 的已知限制）。
  - 没发出的输入（文字和粘贴的图片）按 Session 保留，切换 Session、重连、重启应用后仍在；存在 localStorage（`codeck.connect.drafts`，旧版的 `codeck.drive.drafts` 读一次后删掉，只存有 Session id 的，空的不存；图片预览超出存储上限时只存文字），发出后清掉。
  - 输入框聚焦时 `j` / `k` 等单键快捷键不响应；Esc 在卡片里的输入框中只离开输入框，在输入区以外或 idle 时保持原有含义（关闭 Subagent 面板等）。
- **请求卡片**：样式同对话里的「向你提问」卡片：等待色边框；顶部一条标题栏（图标、标题、Subagent / AI 识别标记，右侧写在等什么：等待审批 / 等待选择 / 等待确认 / 等待处理），下面是正文。选项为整行、等宽的行，左侧是数字键标记；对话框的默认项（审批的「允许」、选择的第一项）画成选中样式，回退的选项都不预先选中，键盘焦点所在行同样画成选中。选项下方写「按数字键作答」。

  | 事件 | 卡片 | 作答 |
  |---|---|---|
  | `permission_request` | 工具名 + 工具输入（用对话记录的工具渲染，展开显示，状态为「等待审批」）；后台 Subagent 的标出 agent。computer use 的授权框（`request_access`）也是这张卡片 | 按 `options`：允许 / 总是允许（按钮显示审批框原文）/ 拒绝；允许不是单纯的 `Yes…` 时按钮也显示原文（computer use 的 `Allow for this session (4 apps)`：窄窗格下列不下的应用不在这次授权里）；选项下方常显一行「拒绝，并告诉 Claude：」输入框，写了字回车即拒绝并带上说明（`deny` + `message`）。`options` 为 null 时没有「总是允许」 |
  | `question` | 主线的提问直接在对话里那张「向你提问」卡片上作答（header 标签与题目、整行的选项、「其他」行，单选 / 多选；单选为圆点、多选为方框，「其他」行同样带标记。单选题里所选项与「其他」互斥：在「其他」里输入会取消已选项，再点选项则「其他」里的字保留但划掉、不随回答发出，点回输入框重新启用；多选题里「其他」是又一个勾选框，可与选项一起勾选、一起发出，点它的方框取消则字保留但划掉），输入区只显示一行提示与「跳到提问」；后台 Subagent 的提问不在主线对话里，在输入区显示同样式的卡片 | `answer_question`（接入前就弹出的提问由 ccrpc 按屏幕作答） |
  | `choice` | 标题 + 选项（有说明时写在选项文字后） | `answer_choice`（按 label） |
  | `dialog` / `unrecognized` | 只读等宽显示屏幕原文（`/cost` 这类面板也在这里），注明其他操作需在 Mac Client 的终端里处理 | 「关闭（Esc）」：busy / waiting 时 `abort`，否则 `send_keys` Esc；状态离开 waiting 即消失 |
  | `feedback_draft` | 输入框上方的卡片：「Claude 起草了一条产品反馈：<标题>」，注明草稿只存在本机、在终端里按 2 才会发送；「查看全文」展开草稿（Markdown） | 「不发送」：`dismiss_feedback_draft`，只在空闲时可点（本轮进行中终端隐藏这张卡片）；草稿留在 `/feedback` 里；CoDeck 不提供发送 |
  | `rewind` 的响应 | 「回退到这条提示词之前」（见下方「回退」） | 「恢复代码与对话」/「只恢复对话 · 代码保持现状」/「只恢复代码 · 对话保持现状」（只列确认页上有的，依次编号）：`answer_rewind {option}`；「取消（Esc）」：`answer_rewind {cancel}` |

  - `layout_source` 为 `learned` 的卡片标「AI 识别」。waiting 但还没有请求（分析器在分析）时显示「正在识别界面…」。
  - 多个请求竖向叠放，主线在上；作答成功即收起该卡片。
  - 数字键 1/2/3 选第一张卡片的选项，只在焦点位于输入区内且不在输入框里时（例如 waiting 时在输入框按 Enter，焦点移到卡片之后）；焦点在对话记录、Subagent 面板等处时按数字键不作答——卡片不是主视图，读对话时误按不能批准工具调用。
- **回退**（ccrpc `rewind` / `answer_rewind`，即终端里的 `/rewind`）：回到 Main Line 上某条提示词之前，可选恢复代码与对话、只恢复对话、只恢复代码。
  - 入口：已连接时，对话记录里 Main Line 上人输入的提示词与斜杠命令、`!` 命令（气泡；命令输出、系统消息与通知不算；终端回退列表里没有的本地命令，如 `/context`，点了报「终端的回退列表里没有这条提示词」）下方一行，悬停时在复制左边出现「回退」；只在空闲时可点，否则禁用并悬停提示「空闲时才能回退」。正在看另一个 Branch 时，分叉点之后的提示词没有；已回退、新 Branch 还没进索引的那段也没有（ccrpc 对它们报 `not_found`）。忙时插话成的提示词（transcript 里的 `queued_command`）在索引里与提示词无从区分，也显示「回退」，但 ccrpc 不认（报 `not_found`），点了由错误卡片写明原因。
  - `/rewind`（或别名 `/checkpoint`，可带参数）：空闲时在输入框发出它不发给终端，进入「选回退点」：输入框上方一张卡片「选一条提示词，回退到它之前 · 点提示词下方的「回退」 · Esc 取消」与「取消」，每条可回退的提示词常显「回退」，对话记录滚到最后一条（闪一下）。「取消」、发出别的内容、点了「回退」、Session 不再空闲或切换 Session 时退出；Esc 也退出，除非这次 Esc 先关掉了别的东西（菜单、⌘K 搜索、⌘F、Subagent 面板、斜杠命令列表）。
  - 点「回退」发 `rewind {uuid}`：进行中按钮写「回退中…」，输入框上方显示「正在准备回退…」（ccrpc 在终端的选择器里找这条提示词，期间不显示「正在识别界面…」，Esc 与「中止」不起作用，以免打断它）；失败时错误卡片写「回退失败：<原因>」。成功后显示确认卡片（与审批、选择卡片同一位置），标题「回退到这条提示词之前」：引用这条提示词（最多 3 行）；一行写代码会怎样（`The code will be unchanged.` →「这条提示词及之后的对话会被移除；代码不变」，`The code will be restored <变化> in <文件>.` →「这条提示词及之后的对话会被移除；代码会恢复：<文件>（<变化>）」，减号写作 −；读不出时只写前半句，其他说法照原文）；有 `warning` 时以警告色写「恢复的文件整份覆盖，之后的改动都会丢失；Bash 对其他文件的改动不会撤销。」。卡片第一次出现时，若焦点还在点下的「回退」上或不在任何地方，焦点移到卡片本身（不落在任何选项上，正在输入时不动），之后数字键与 Esc 同其他卡片（Enter 在输入框里把焦点移到第一个选项）。Esc 与输入框的「中止」在这张卡片上都是取消回退，不发 `abort`；同一张卡片同时只发一次 `answer_rewind`，作答进行中再按 Esc、「中止」或选项不再发送。状态离开 waiting（在终端里作答或按了 Esc）、或来了别的请求 / 对话框时卡片消失；`rewind` 的响应到达时 Session 已经等待过又不再等待（确认页已在终端里被作答或退出），不显示卡片。`answer_rewind` 报 `unknown_request` / `not_waiting`（确认页已不在屏幕上）时卡片也收起，错误卡片写原因。
  - 恢复了对话（「恢复代码与对话」「只恢复对话」）：输入框为空（没有文字也没有图片）时填入 ccrpc 返回的原提示词并聚焦输入框；已有内容时保留，输入框下方一行「输入框里已有内容，原提示词没有填入」（从 CoDeck 发出提示词后去掉）。transcript 要等下一条提示词才体现回退，在那之前对话记录里这条提示词起到末尾淡化（不透明度 .38，仍可展开查看），上方一条蓝色分隔线「已回退到这里，代码已恢复 · 发出新提示词后，下面这段成为另一个 Branch」（只恢复对话时没有「，代码已恢复」）；只在看 Main Line 时显示。标记与 ccrpc 记住的回退点同步，属于这个连接：忙起来（后台任务的通知轮不分叉）、切换到别的 Session 再回来都还在；这条提示词已不在索引的 Main Line 上（发出的新提示词分叉了）、`session_changed`（`/clear` 等）、Session 结束、ccrpc 退出或 Disconnect 时清除，之后由已有的 Branch 显示接手。
  - 只恢复代码：对话不变，通知卡片「已恢复代码」（可关）。
  - 挂起的回退属于发出 `rewind` 的那个 ccrpc：重连后的新 ccrpc 不知道它，终端里的确认页照其他对话框处理（CoDeck 不调用 `get_state`）。
- **Session 列表**：顶部「新建 Session」（不依赖 `ccrpc ls`）：选目录 → `ccrpc new --cwd`，`ready` 后选中它；失败时列表顶部显示一行错误。新 Session 发第一条提示词前没有 transcript 文件。`ccrpc ls` 里有、还没进索引的 Session（在 clay 里刚打开的，或刚新建的）以占位行列在列表最上方，不参与排序：标题「新 Session」、Live 点（按 `ccrpc ls` 的 status）、cwd 的最后一段；Project 筛选时按 cwd 所在的 Project（路径最深的那个）匹配，匹配不上不显示，从统计下钻时不显示。选中后标题栏显示「新 Session」、cwd 与「Connect」，主区显示「新 Session，发送第一条提示词后显示对话」，输入区照常可用；进索引后占位行变成正常的行，主区自动切换为正常显示。已连接的 Session 在 `turn_end` 后没看过时显示未读点，看过即清。
- **`session_changed`**（在 TUI 里 `/clear` 等）：正在看旧 Session 时，选中切换到新 Session。
- **别处在等**：切回窗口时，当前 Session 没有要作答的、而别的 Session 在等作答（与 Dock 角标同一口径），对话记录顶部显示一行「<标题> 在等你作答，点此切过去」，点击选中那个 Session；切换 Session 后消失，可关闭。
- **Dock 角标**：等待作答的 Session 数 = Live 状态为 waiting 的 Session（全部 Session，不限于已连接）∪ 已连接且有请求卡片的 Session，去重。
- **与 cc-gui 不同之处**（有意不移植）：等作答的 Session 不在列表里置顶，Session 列表保持原有排序（Live 点为琥珀色）；`ccrpc doctor` 的 fail 不在启动时顶部提示，只在「设置 › ccrpc」里显示（启动时不读 ccrpc 信息）。
- **设置 › ccrpc**：ccrpc 路径、`ccrpc doctor` 的结果（fail 醒目显示）、界面分析器、启动参数、日志目录。这些信息在打开该页时才读取。界面分析器（ccrpc 认不出的对话框交给它，Anthropic Messages 协议，`POST {Endpoint}/v1/messages`）：显示是否开启、Key 来自哪里（设置 / 登录 shell，只显示末 4 位）；可填 API Key、Endpoint、模型，存进应用的 `settings.json`（`analyzer`，文件权限 0600），启动 ccrpc 时覆盖登录 shell 的 `CCRPC_ANALYZER_API_KEY` / `_BASE_URL` / `_MODEL`，留空的项照用登录 shell 的或 ccrpc 的默认值；对之后新建的连接生效。Key 输入框留空保存不改已存的 Key，「清除保存的 Key」删掉它。「测试」用框里当前的值（不保存；Key 留空时用已存的或登录 shell 的，留空的 Endpoint、模型同保存后的取法）运行 `ccrpc analyzer test`，显示「可用：<模型> 用时 N 秒」或失败原因；哪里都没有 Key 时不可点。
- **启动参数**（设置 › ccrpc）：按目录追加给 claude 的参数，相当于按工作目录加 `--add-dir` 的 shell 包装函数。claude 以 `command claude` 启动，shell 里同名的包装函数、别名不起作用，CoDeck 给 claude 的参数只来自这里。每条是「目录（选取）+ 参数」，存进 `settings.json` 的 `launchRules`；「新建 Session」时取包含所选目录的最深一条（按路径逐级比较，`/a/b` 不包含 `/a/bc`），Resume 时同样按启动目录取规则。参数按 shell 规则拆分（引号成组；`~` 在词首或词里第一个 `=` 之后、`$VAR` / `${VAR}` 按登录 shell 的环境展开，单引号里的不展开）后以 `ccrpc new … -- <参数>` 传给 claude。保存时去掉首尾空白、丢掉参数为空的条目；原样保存所写的参数；目录不是绝对路径、重复、引号不配对或用到登录 shell 里没有的环境变量时不保存并显示原因。对之后新建的 Session 与 Resume 生效。
- **隐藏 Pane**：Session 在 clay 里运行（在 `ccrpc ls` 里，不论是否 Connect、是不是 CoDeck 开的）时，标题栏在「Connect」左边显示「隐藏 Pane」（`ccrpc hide <Holder ID>`：Pane 收进 ToH Mac Client 的隐藏窗口，iOS 与 web 照常显示，ToH ADR-0050），已隐藏的显示「显示 Pane」（`ccrpc show`）；状态取 `ccrpc ls --json` 的 `hidden`，点下后立即改，失败时标题栏写「隐藏 Pane 失败：<原因>」。ccrpc 太旧、`ls` 没有 `hidden` 时不显示。「设置 › ccrpc › 新开的 Pane」选「显示」（默认）或「隐藏」，存进 `settings.json` 的 `hideNewPanes`，点下即保存；选「隐藏」时新建 Session 与 Resume 都以 `ccrpc new --hidden` 启动。claude 退出后 Pane 照常留在 shell 里，不自动关闭。
- **退出应用**：关掉每个 ccrpc 的 stdin（stdin 被卡住的写入占着时发 SIGTERM），ccrpc 做完当前命令后 Detach 退出，Session 照常运行；应用崩溃时管道随之关闭，效果相同。卡死、做不完当前命令的 ccrpc 会留下，它占着这个 Session，下次 Connect 时 ccrpc 报「已有 ccrpc 连着这个 Session」，结束那个进程后即可再连。
- **日志**：常开，按连接分目录，仅本人可读写，保留 14 天：

  ```
  ~/Library/Logs/dev.joy.codeck/ccrpc/<日期>/<holder_id>-<时刻>/
    wire.jsonl   与 ccrpc 之间的每一行原样（命令、响应、事件）；超过 50 MB 轮转为 wire.1.jsonl
    ccrpc.log    ccrpc --log
    gui.log      用户操作（按钮 → 命令 id）、失败的命令、ccrpc 的退出码与 stderr
  ```

## 测试

- 解析器的回归测试使用 fixture：从真实 Session 精简后脱敏（保留结构，文本替换成占位内容），覆盖 Branch、Fork、compact、Subagent、Workflow、后台 agent、失败的调用、图片等情况。
- E2E：Playwright 驱动前端并 mock IPC（`tauri-driver` 不支持 macOS）；Rust 后端单独测试。
- Connect 的 E2E 用 wire 回放的 mock ccrpc：`src/mocks/data/connect/*.jsonl` 是录下（或手写）的 `wire.jsonl`，收到命令回录下的响应，事件按录下的顺序推；`meta` 行里的 `session_id` 把录制对应到 mock 数据里的 Session，`connect_delay_ms` 让连接命令晚于事件返回（复现 ccrpc 秒退时退出事件先到），`on_request` 让它默认不在 `ccrpc ls` 里、测试调用 `__cvCcrpc.list(<名字>)` 后才列出（`rewind.jsonl` 对应的 Session 在别的用例里是不在 clay 里的那个）。
- 启动、后台调度、配置类流程必须在真实应用中验证（单测覆盖不到，例如曾出现的全文索引任务互相让位导致永不执行）。

## 技术栈

React + TypeScript + Vite；Tailwind + shadcn/ui（macOS 风格，跟随系统深浅色）；TanStack Virtual；react-markdown + remark-gfm；Shiki（在 Worker 中运行）；`diff`；ECharts；specta + tauri-specta；rusqlite（元数据、统计、FTS5）。

## 里程碑

| | 内容 | 状态 |
|---|---|---|
| **M0 设计** | 用 Claude Design 出设计稿：主界面（浅色 + 深色）、Subagent 面板、⌘K 搜索、统计面板、状态类画面（首次索引、空结果、诊断页）。使用合成数据，macOS 原生质感，以浅色为主 | 已完成 |
| **M1 骨架** | 扫描和增量索引、FSEvents 监听、去重与合并、三栏布局、基础对话渲染、Live 标记和实时追随、宽容解析、fixture 测试、Session 头部元信息 | 已完成 |
| **M2 读得舒服** | 工具渲染器、工具调用分组、Subagent 和 Workflow、Branch、Fork、compact、系统消息开关、图片、`tool-results` 按需加载 | 已完成 |
| **M3 找得到** | FTS5 索引、⌘K 搜索和过滤器、命中定位、工具输出扫描、⌘F、`j`/`k`、轮次大纲、复制、在 Finder 中显示 | 已完成 |
| **M4 统计与诊断** | 列表 token 列、统计面板、格式兼容性诊断页 | 已完成 |

## 已知限制

以下问题在验收时识别，因不影响结果正确性而暂缓：

- 同一个 Session 被并发请求（例如实时追随与查找同时进行）时，可能各自重新解析一次文件。
- 每批文件变更都会重新遍历数据目录（真实数据约 60 ms）。
- 某个 Session 的 Main Line 持续计算失败时，该 Session 不建全文索引，搜索浮层会一直提示索引未完成。
- sidecar（`meta.json`、workflow json）比对应转录更新的 Session，每次启动都会重算一次。
- 无斜杠前缀的历史命令记录，标题要等文件重新解析后才会更新。
