# CoDeck 第一版规格

术语见 [CONTEXT.md](../CONTEXT.md)，关键取舍见 [docs/adr/](./adr/)。

## 定位

macOS 桌面应用，用来查看 `~/.claude` 下 Claude Code 的历史 Session，并驾驶 ToH clay 里运行中的 Session。查看的核心场景有两个：**找回某次对话**（搜索、定位）和**复盘某个 Session**（读得舒服）；统计是附加功能。除经 ccrpc 驾驶（见「驾驶」一节，ADR-0007）外不对 Session 做任何写操作；数据目录始终只读（ADR-0002）。

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
- **Branch / Main Line**：消息按 parentUuid 构成一棵树。Main Line 是走到最新叶子的那条路径（优先用 `last-prompt.leafUuid`，否则取时间戳最新的叶子）。
- **compact**：compact 边界会切断 parentUuid 链；边界通过 `logicalParentUuid` 接回压缩前的历史，但只接受文件中位置更早的条目（否则会成环），找不到时接到文件中前一条消息，因此 Main Line 跨越 compact 保持连续。
- **Fork 判定**：条目带 `forkedFrom` 的即为 Fork，继承部分以此标记；没有该字段时，以和更早的 Session 共享首条消息作为兜底。
- **Live Session**：`~/.claude/sessions/<pid>.json` 存在、进程存活且 `procStart` 一致；`status` 区分 busy、waiting 和 idle。兜底规则：文件最近几分钟内被修改过。
- 标题优先级：`custom-title` > `ai-title` > 第一条用户输入。
- **用户输入的判定**：只有用户亲手输入的内容和斜杠命令算作"你"的消息；系统注入、任务通知、命令输出、compact 摘要和 Teammate Message 都不算，不参与消息数、分支、标题和"你"角色的搜索。Claude 工作期间输入、排队到下一轮才送达的内容，文件里只记为 `queued_command` 附件（没有对应的 `user` 条目），按同样的规则当作用户消息处理：亲手输入的算"你"的消息，排队的任务通知、Teammate 消息按各自类型显示。新版 Claude Code 把 `/rename`、`/model` 这类本地执行的斜杠命令及其输出记成 `system`（`local_command`）条目，旧版记成 `user` 条目，两种按同样的规则处理：命令算"你"的消息，输出按命令输出显示，空输出隐藏。长段粘贴被 Claude Code 包在 `<pasted_content id="xxxx">` 标签里时（账号级开关），显示、标题和搜索都用去掉标签后的原文。
- **计数口径**：消息数 = 用户输入 + assistant 消息（按 `message.id` 合并拆分的片段），只计主对话；token 按 `message.id` 只计一次，每个字段取各片段的最大值；Session 的 token 合计包含其 Subagent。同一条 assistant 消息出现在多个 Session 中时（resume / Fork 会把之前的对话复制进新 Session 的文件），token 和 API 金额只计入最早创建的那个 Session，统计也只计一次（与 ccusage 一致）；Session 被删除后，归属转给下一个仍包含它的 Session。消息数不受影响。
- **API 金额**：参照 ccusage 的算法，按 LiteLLM 价格表逐条计算 assistant 消息：输入、输出、缓存读取、缓存写入各按单价计；缓存写入区分 5 分钟和 1 小时（1 小时按价格表中的单价，没有时按输入单价的 2 倍）；模型有 200K 以上单价时，各项超过 200K 的部分按该单价计；fast 模式乘以模型的 fast 倍率；条目自带 `costUSD`（旧版 Claude Code）时直接采用。模型名去掉 `[1m]`、`@地区`、日期后缀再匹配；匹配不到的消息不计入金额并单独计数，界面上提示"无价格"。
- **价格表**：内置一份 LiteLLM 中 `claude-*` 模型的快照；启动时以及之后每 10 分钟检查一次，缓存超过一天就在后台重新下载 LiteLLM 价格表（ADR-0005），存到 Caches 下的 `pricing.json`，价格有变化时重新计算全部金额。下载失败沿用缓存或内置快照，不提示。
- 数据根目录：默认 `~/.claude`，识别 `CLAUDE_CONFIG_DIR`，也可以在设置里修改；只支持一个数据根目录。不读取 `~/.claude/transcripts/`。

## 索引

- 可以丢弃的镜像缓存，存放在 `~/Library/Caches/<bundle-id>/`；源文件删除后对应 Session 随之消失；schema 变化时直接重建（ADR-0003）。
- 增量方式：按文件记录已读到的字节偏移；文件变小或 inode 变化时整个文件重新解析。用 FSEvents 监听变化。
- 首次启动先扫描元数据，几秒内列表就能用；全文索引在后台分批构建，状态栏显示"正在建立全文索引 · x / y 个文件"；文件变更会插队处理，之后索引继续。实测 3 GB 数据约 2–3 分钟建完，索引约 760 MB。
- 宽容解析：以行为单位，失败的行跳过并计数；未知的条目类型、内容块、工具都用通用方式显示，不丢内容；所有字段按可选处理。

## 搜索

- 入口：⌘K 或红绿灯右侧的搜索图标打开搜索浮层，再按 ⌘K 关闭；不用双击 Shift，以免和输入法切换冲突。不设过滤器条。
- 语义：字面子串匹配，支持多词 AND、`"短语"`、`-排除词`；Session、Project 和对话内容用同一套规则，只有排除词时不匹配。
- 结果固定分三段，后到的结果只追加在下方，已选中的项不跳：
  - **Project**（最多 3 个）：名称或路径命中。↵ 用它筛选 Session 列表（列表收起时自动展开）；⌘↵ 把它设为搜索范围，显示为输入框前的标签，三段都只在其中，输入为空时 ⌫ 移除。
  - **Session**（最多 8 个）：标题、Project 名、git 分支命中，在前端即时匹配，不等防抖；命中 Project 名或分支的词越多越靠前，其余按最近活跃。空输入时列出最近的 Session（最多 20 个）。
  - **对话内容**：全文索引，200 ms 防抖后出现，按 Session 分组，每组最多 5 处。
- ↑↓ 在各段之间连续移动，⇥ / ⇧⇥ 跳到下一段 / 上一段的第一项；打开的 Session 不在当前 Project 筛选里时清除筛选。关闭后保留输入和范围，再打开时全选输入框。
- 默认范围：用户输入、Claude 回复、工具输入，用 SQLite FTS5 trigram 索引；AskUserQuestion 的回答和备注按"你"的角色索引，打开命中定位到对应的问答；少于 3 个字符的词（如两个汉字）改用子串匹配，这种情况下只对 ASCII 字母忽略大小写。
- 工具输出不建索引，也不搜索（ADR-0004）。
- 输入法组字期间不过滤也不发起搜索，确认上屏后才搜索（⌘F 同理）；组字时按 Esc 只取消候选词，不关闭搜索或查找。
- 不建索引：attachment、hook 输出、系统注入、图片、元数据。
- 对话内容按 Session 分组，按时间倒序；全文索引未建完时提示进度，Session 与 Project 的匹配不受影响；命中位置在 Subagent 里时标注其 agentType，在已回退的 Branch 里时标注"已回退的分支"；打开命中会自动切换分支、打开 Subagent 面板并定位高亮。
- 不做语义搜索，但保留以后扩展的可能。

## 界面

- 两栏布局：Session 列表 | 对话区；红绿灯右侧是 Session 列表开关和搜索图标（悬停提示 ⌘K），全屏时没有红绿灯，两个图标移到最左边；对话头部不另设搜索入口。状态栏右端是统计按钮和设置按钮（⌘,）。
- 统计以大弹窗盖在窗口上显示，点弹窗外、Esc 或右上角关闭按钮关闭；从统计下钻或打开 Session 时自动关闭；打开搜索时也会关闭统计。
- Session 列表顶部：红绿灯那一行只放上述两个图标；下一行是「全部 Session」与数量，右侧是排序（如「按时间 ↓」，菜单切换排序字段和正序/倒序）。从搜索里按 Project 筛选时，标题位置显示 Project 名，× 清除。
- 打开 Subagent 面板时，Session 列表栏让出宽度。
- Session 列表可收起（⌃⌘L），收起状态在重启后保留；Session 列表收起时在搜索里选择 Project 会自动展开它；Session 列表被 Subagent 面板占用时其开关不可用。
- 各栏头部、对话头部和底部状态栏可以按住拖动窗口（按钮、输入框除外）。
- 窗口整体不随触摸板回弹，只有各栏内容区自身滚动。
- 滚动条只在滚动时或指针停在滑块上时显示，不受系统"显示滚动条"设置影响。
- Session 列表每行显示：标题、相对时间、消息数、token 数、API 金额（时间、消息数、token、金额都可以排序）、Live 标记、git 分支；从统计下钻时，列表顶部显示可清除的筛选标签。
- Session 头部显示：模型、token（输入/输出/缓存）、API 金额（含 Subagent；悬停显示按模型的明细，有无价格的消息时金额后带"+"）、时长、消息数、工具调用次数、Subagent 数。
- 对话渲染：
  - 每个工具调用折叠成一行摘要；连续的工具调用合并成一组；失败的调用和工具组同样默认折叠，组头显示失败数。
  - 专用渲染器：Bash（终端风格，支持 ANSI）、Edit（diff）、Write、Read（默认不展开内容）、Task 清单；其他工具用通用 JSON 视图。
  - AskUserQuestion 不并入工具组、不折叠：一张卡片列出全部问题和选项，就地标出所选项（含"其他"里输入的内容）和备注，回答不再另外重复显示；卡片头部注明"你 · 时间 已回答"、等待回答或未回答（被拒绝或调用失败）；用户选择先聊聊（Chat about this）时文件里记为拒绝，卡片按正常样式显示"你 · 时间 选择先聊聊"，并标出此前已答的问题。回答不计入消息数，不开启新的轮次。
  - 思考块：有内容时折叠显示，空的隐藏。
  - attachment 和系统注入默认隐藏，可以用开关显示。
  - compact 边界显示为分隔线；`tool-results/*.txt` 按需加载；图片显示缩略图。
  - Branch 用 `‹ i/n ›` 切换；如果被放弃的分支只是原样重发且后面没有回复，不显示切换器。
  - 斜杠命令像终端一样高亮：命令名用等宽强调色，参数保持正文字体。
  - Teammate Message 显示为可折叠的"Teammate · <id>"卡片，不显示为用户气泡。
  - 当前 Session 读取失败时显示一行错误；源文件被删除时回到空状态。
- Subagent：主对话中显示为卡片（任务 + 结果）；"查看完整过程"在右侧面板打开，嵌套时显示面包屑；后台 agent 的卡片和结果通知互相链接；Workflow 显示为一张总卡片。
- 阅读便利：⌘F（会自动展开折叠的内容，包括工具组内的思考块和截断的工具输出）、`j`/`k` 按轮次跳转（对话框打开时不响应）、轮次大纲、复制消息/代码块/Bash 命令、复制 Session ID 和 `claude --resume <id>`、在 Finder 中显示。
- 不做：导出、URL scheme、书签和笔记、汇率换算、驾驶以外任何写 Session 的操作。
  - 导出为 HTML 评估过，暂不做：对话列表是虚拟列表，导出要另行整页渲染；按需加载的工具输出、图片要一次性取全；Shiki 高亮是异步的，要等全部完成；折叠需改成 `<details>`；保存位置需引入对话框插件。只导出 Main Line、工具保持摘要与预览的首版约 1–2 天，带完整工具输出和 Subagent 过程会更多。

## 统计面板

时间范围；概览卡片（Session 数、消息数、Token、API 金额、活跃天数）；每日 token 柱状图；活跃时段；Session 排行（按 API 金额，最多 10 个，在右侧跨两行）。不统计工具调用和 Subagent。页面背景铺满窗口高度，图表保持固定高度，不随窗口拉高；头部（标题、时间范围）固定，只有下方内容滚动。不按 Project 过滤或排行。

- Token 卡片占两格：输入与输出两个数分居左右两栏（位置不随数值长短变化），右上角缓存命中率，下方一条构成条和图例（缓存读取 / 缓存写入 / 未缓存输入，悬停说明各自含义与计价倍率）。输入 = 未缓存输入 + 缓存写入 + 缓存读取；缓存命中率 = 缓存读取 ÷ 输入。
- 每日 token 可切换「全部 / 输入 / 输出」：全部是每天输入、输出两根柱子；输入、输出按模型堆叠，图例可下钻。
- Session 日均数不足 10 时保留一位小数。
- 活跃时段是时间直方图：每根柱子是一段时间内主对话的消息数，柱宽从 1h、3h、6h、12h、1d、1w、1mo 中自动选最细、且柱子不超过 170 根的一档（7 天 → 1h，30 天 → 6h，90 天 → 1d，「全部」按从最早一条消息到现在的跨度）；按本机时区对齐（整 n 小时、0 点、周一、1 号），没有消息的柱子为 0；夏令时跳过的小时没有柱子，回拨重复的小时并入同一根柱子；按月仍超过 170 根时只显示最近 170 个月。卡片右上角标柱宽，没有模式切换；坐标用英文和数字，悬停提示用中文。点击柱子把 Session 列表筛到这段时间。

- 时间范围：本周期 / 7 天 / 30 天 / 90 天 / 全部（默认 30 天）。"本周期"从最近一次每周重置算起到现在；重置时间（星期几 + 时刻，本机时区，默认周一 0:00）在「设置 › 通用 › 统计周期」里设置，重启后保留；选中本周期时，标题同一行显示本周期的起止时间和剩余时间。切换时间范围时页面布局不变（时间范围按钮选中时不加粗，以免宽度变化）。
- 口径：消息数和活跃时段只计主对话；token 和 API 金额（概览、每日、Session 排行）包含 Subagent，跨 Session 重复的消息只计一次；排行里的金额只算时间范围内的消息。日期按本机时区划分。
- 下钻：点击每日 token 的柱子、活跃时段的柱子、模型图例会筛选 Session 列表，并带上当前的时间范围；点击 Session 排行打开该 Session。

## 诊断

在设置（⌘,）的"格式兼容性"页：解析失败的行（含文件与首个错误）、未知的条目类型 / 内容块 / 系统子类型 / 工具名及出现次数和涉及版本、各 Claude Code 版本汇总、文件内重复 uuid 数、未关联的 Subagent 数；可复制报告、重建索引。

## 设置

数据根目录按"设置中的覆盖 → `CLAUDE_CONFIG_DIR` → `~/.claude`"确定；覆盖值保存在 `~/Library/Application Support/<bundle-id>/settings.json`，切换后清空当前视图并重新扫描。

## 驾驶

经 ccrpc（[crates/ccrpc](../crates/ccrpc)，协议见其 `docs/spec.md` §5）控制 ToH clay 里运行、装了 ccrpc 插件的 Session（ADR-0007）。**对话内容仍来自本应用的索引与实时追随，ccrpc 只提供状态与控制**：不调用 `get_messages`，不用 ccrpc 的内容块、`branch`、`workflow` 事件（内容块只用来知道某次工具调用已执行、对应的请求结束）。Rust 后端（`src-tauri/src/drive`）只管 ccrpc 子进程、转发 JSONL、写日志；协议解析与状态在前端（`src/features/drive`）。术语另见 ccrpc 的 [CONTEXT.md](../crates/ccrpc/CONTEXT.md)（受控 Session、请求、选择、轮）。

- **环境**：第一次用到驾驶时读一次登录 shell 的环境（`$SHELL -l -c env`），作为所有 ccrpc 子进程的环境，不在启动时读，以免拖慢启动。ccrpc 依次取环境变量 `CODECK_CCRPC`、`~/.local/bin/ccrpc`、PATH 里的 `ccrpc`。
- **可驾驶的 Session**：窗口可见、且至少有一个 Live Session 或驾驶连接时运行 `ccrpc ls --json`，上一次返回后隔 2 秒再运行下一次；条件不满足时停止（没有 Live Session 也没有连接时丢掉上次的结果），条件恢复时立即查一次再继续；停止前发出、停止后才返回的结果丢弃。Holder 的 `session_id` 就是 Session ID。`ccrpc ls` 或读取 ccrpc 信息失败时停止轮询，错误显示在「设置 › 驾驶」，可重试。
- **标题栏**：选中的 Session 在 `ccrpc ls` 里且插件已报到时显示「驾驶」（`ccrpc attach --co-display`）；插件未报到时按钮禁用，悬停提示「ccrpc 插件未加载，重启 claude 后可驾驶」；不在列表里（不在 clay 里，或 ccrpc 不可用）时不显示。已连接时「驾驶」按钮消失，标题栏在元信息行下方另起一行，显示状态、模型、权限模式（可点，`set_mode`；空闲或忙时可切换）、上下文 token、悬停出现的「复制排查信息」（holder_id、session_id、cwd 与这个连接的日志目录）和「断开」（关闭 ccrpc 的 stdin，Session 照常运行）。连接保持到断开、Session 或 ccrpc 退出、应用退出。
- **驾驶区**：在对话区下方、与对话区同宽，只在当前 Session 已连接时出现。自上而下：ccrpc 意外退出的横幅（退出码与 stderr 最后一行 +「重连」，不自动重连）、Session 已结束、通知（follow_up 没发出、学会新界面、界面识别失败、被接手 / 取回，可关）、请求卡片、忙时的进度行（否则为上一轮的用时、token、API 金额、错误、已中止）、排队的 follow_up（可撤销）、输入框（粘贴的图片、后台任务角标，点开可停止）。
- **输入框**：

  | 状态 | Enter | 其他 |
  |---|---|---|
  | idle | `prompt` | |
  | busy | `follow_up` | ⌘Enter `steer` |
  | waiting | 不发送，焦点移到第一张请求卡片 | |
  | busy / waiting | | Esc `abort`（焦点在驾驶区时） |

  - 粘贴的图片存到 `~/Library/Caches/dev.joy.codeck/images/`，输入框上方以 `data:` 地址预览，路径作为 `images` 传给 `prompt` / `follow_up`；`steer` 不带图片。
  - 斜杠命令：idle 时内置命令（以及补全清单里没有、形如命令名的名字）走 `slash`——本地命令和打开面板的命令在 transcript 里不留可等的记录；自定义命令、skill、`/Users/…` 这类路径、多行或带图片的，以及取不到补全清单时，走 `prompt`。忙时照常 `follow_up`，但会打开界面的内置命令不发送，提示等空闲时再发（排队后会以 `prompt` 发出，等满 30 秒）。
  - 补全：输入框只有 `/名字`（还没有空格、换行）时，上方列出匹配的命令（名字完全相同 > 前缀 > 命名空间 `:` 之后的前缀 > 包含）；只输入 `/` 时列出全部，列表可滚动，显示描述、来源（内置 / 用户 / 项目 / 插件），会在终端里打开界面的内置命令标「打开界面」；↑↓ 选择，Tab 补全，Enter 在选中项与输入不同时补全、相同时发送，Esc 只收起列表。清单来自内置命令表、数据根下的 `commands/`、`skills/`，Session cwd 下的 `.claude/commands`、`.claude/skills`，以及用户和项目设置里启用的插件的 commands / skills（插件的以 `插件名:` 为前缀）；只读，同名时前者优先。
  - 共显时写入会清空 TUI 输入框里没发的内容（ccrpc 的已知限制）。
  - 没发出的输入按 Session 保留，切换 Session、重连后仍在。
  - 输入框聚焦时 `j` / `k` 等单键快捷键不响应；Esc 在卡片里的输入框中只离开输入框，在驾驶区以外或 idle 时保持原有含义（关闭 Subagent 面板等）。
- **请求卡片**：

  | 事件 | 卡片 | 作答 |
  |---|---|---|
  | `permission_request` | 工具名 + 工具输入（用对话区的工具渲染，展开显示，状态为「等待审批」）；后台 Subagent 的标出 agent | 按 `options`：允许 / 总是允许（按钮显示审批框原文）/ 拒绝；另有「拒绝，并告诉 Claude…」（`deny` + `message`）。`options` 为 null 时没有「总是允许」 |
  | `question` | 题目、单选 / 多选、「其他」文本框 | `answer_question` |
  | `choice` | 标题 + 选项按钮 | `answer_choice`（按 label） |
  | `dialog` / `unrecognized` | 只读等宽显示屏幕原文（`/cost` 这类面板也在这里），注明其他操作需在 Mac Client 的终端里处理 | 「关闭（Esc）」：busy / waiting 时 `abort`，否则 `send_keys` Esc；状态离开 waiting 即消失 |

  - `layout_source` 为 `learned` 的卡片标「AI 识别」。waiting 但还没有请求（分析器在分析）时显示「正在识别界面…」。
  - 多个请求竖向叠放，主线在上；作答成功即收起该卡片。
  - 数字键 1/2/3 选第一张卡片的选项，只在焦点位于驾驶区内且不在输入框里时（例如 waiting 时在输入框按 Enter，焦点移到卡片之后）；焦点在对话区、Subagent 面板等处时按数字键不作答——卡片不是主视图，读对话时误按不能批准工具调用。
- **Session 列表**：顶部「新建 Session」（不依赖 `ccrpc ls`）：选目录 → `ccrpc new --co-display --cwd`，`ready` 后选中它；失败时列表顶部显示一行错误。新 Session 发第一条提示词前没有 transcript 文件。`ccrpc ls` 里有、还没进索引的 Session（在 clay 里刚打开的，或刚新建的）以占位行列在列表最上方，不参与排序：标题「新 Session」、Live 点（按 `ccrpc ls` 的 status）、cwd 的最后一段；Project 筛选时按 cwd 所在的 Project（路径最深的那个）匹配，匹配不上不显示，从统计下钻时不显示。选中后标题栏显示「新 Session」、cwd 与「驾驶」，主区显示「新 Session，发送第一条提示词后显示对话」，驾驶区照常可用；进索引后占位行变成正常的行，主区自动切换为正常显示。已连接的 Session 在 `turn_end` 后没看过时显示未读点，看过即清。
- **`session_changed`**（在 TUI 里 `/clear` 等）：正在看旧 Session 时，选中切换到新 Session。
- **别处在等**：切回窗口时，当前 Session 没有要作答的、而别的 Session 在等作答（与 Dock 角标同一口径），对话区顶部显示一行「<标题> 在等你作答，点此切过去」，点击选中那个 Session；切换 Session 后消失，可关闭。
- **Dock 角标**：等待作答的 Session 数 = Live 状态为 waiting 的 Session（全部 Session，不限于已连接）∪ 已连接且有请求卡片的 Session，去重。
- **与 cc-gui 不同之处**（有意不移植）：等作答的 Session 不在列表里置顶，Session 列表保持原有排序（Live 点为琥珀色）；`ccrpc doctor` 的 fail 不在启动时顶部提示，只在「设置 › 驾驶」里显示（启动时不读 ccrpc 信息）。
- **设置 › 驾驶**：ccrpc 路径、`ccrpc doctor` 的结果（fail 醒目显示）、分析器是否开启（登录 shell 环境里设置了 `CCRPC_ANALYZER_API_KEY`）、日志目录。这些信息在打开该页时才读取。
- **日志**：常开，按连接分目录，仅本人可读写，保留 14 天：

  ```
  ~/Library/Logs/dev.joy.codeck/drive/<日期>/<holder_id>-<时刻>/
    wire.jsonl   与 ccrpc 之间的每一行原样（命令、响应、事件）；超过 50 MB 轮转为 wire.1.jsonl
    ccrpc.log    ccrpc --log
    gui.log      用户操作（按钮 → 命令 id）、失败的命令、ccrpc 的退出码与 stderr
  ```

## 测试

- 解析器的回归测试使用 fixture：从真实 Session 精简后脱敏（保留结构，文本替换成占位内容），覆盖 Branch、Fork、compact、Subagent、Workflow、后台 agent、失败的调用、图片等情况。
- E2E：Playwright 驱动前端并 mock IPC（`tauri-driver` 不支持 macOS）；Rust 后端单独测试。
- 驾驶的 E2E 用 wire 回放的 mock ccrpc：`src/mocks/data/drive/*.jsonl` 是录下（或手写）的 `wire.jsonl`，收到命令回录下的响应，事件按录下的顺序推；`meta` 行里的 `session_id` 把录制对应到 mock 数据里的 Session，`connect_delay_ms` 让连接命令晚于事件返回（复现 ccrpc 秒退时退出事件先到）。
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
