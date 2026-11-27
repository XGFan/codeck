# agy 的历史：只读接入

CoDeck 现在只读 Claude Code 的数据（`~/.claude`）。这次把 agy（Antigravity CLI）的对话也作为 Session 接进来：列表、对话记录、⌘F、CLI 都能看，只读。路线见 ADR-0018；Connect 另做，不在这份设计里。

## 目标与边界

- agy 的对话是 Session，Session 多一个属性 **Agent**（`claude` / `agy`）；Session ID 是 agy 的 conversation id。
- 标准：允许取舍与 let it crash，用它换实现简洁、易于维护；罕见的情况不加防御，出错时让这一处失败、别处照常。
- 不做：
  - 全文检索：agy 的文件不进全文索引，⌘K 搜不到 agy 的内容。⌘F 会话内查找照常（它查的是组装后的对话）。
  - Connect。
  - agy 子 agent（`invoke_subagent`）产生的子对话与父对话的关联（v1 里它们是普通 Session）。
  - agy 数据目录的设置项（固定 `~/.gemini/antigravity-cli`，不存在就没有 agy 的 Session）。
  - agy 的额度；agy 进统计。
- CoDeck 之外的一处改动：haa 在 `usage.jsonl` 的每条 Task 记录里写上 `sessionId`（PaaA 仓库，单独提交）。没有它 CoDeck 照常工作，只是 haa Worker 连不到 Main。

## 用户看得到的

### 列表与 Session

- agy 的 Session 和 Claude Code 的一起出现在侧栏（Live、Recent、Project）与 Session 列表里；iPhone 经 Host 同样看得到。agy 的 Session 带一个标记，样式与位置先在设计画布上定（见「设计画布」）。
- 标题取 agy 自己生成的标题（`conversation_metadata.json` 的 `Title`，地位同 Claude Code 的 ai-title）；首条提示词取第一个用户 step 里 `<USER_REQUEST>` 之间的文字。
- Project 取第一个 workspace（`WorkspaceURIs[0]`，`file://` URI 解码成路径），按现有规则（ADR-0006）解析成仓库；没有 workspace 时落到 agy 数据目录本身。
- 「···」菜单有「复制 Resume 命令」（`agy --conversation <id>`）；会运行并 Connect 的 Resume 按钮，agy 的 Session 不显示（v1 没有 Connect）。

### 对话记录

- 一条 Main Line：没有 Branch、Fork、compact。
- 用户提示词、回答、思考、工具调用照 Claude Code 的样子显示：agy 的工具译成 Claude Code 的对应工具后交给现有的渲染器，「运行 1 条命令、编辑 2 个文件」的合并、CLI 都照常。工具详情（现有的展开入口）另显示 agy 原来的工具名与参数，这是前端唯一要为对话记录做的改动。
- agy 的系统消息（`SYSTEM_MESSAGE`）按 Claude Code 的系统条目折叠显示；出错的 step（`ERROR_MESSAGE`）显示为错误。

### Live

有 agy 进程打开着 `presence/<id>.lock` 时这个 Session 是 Live。v1 不知道它在忙还是在等，状态是「未知」。

### haa Worker

- 由 haa 起的 agy Session 标为 haa Worker。认法：任一 workspace 在 `~/.haa/` 或 `~/.paaa/` 下（haa 挂 hooks 用的），或调用过 `mcp_haa_*` / `mcp_paaa_*` 工具。本机 188 个 agy 会话里大部分是这种。
- haa 的记录里有这个会话的 id 时，显示它属于哪个 Main Session（可跳转）、Worker 名与 Task 标题；没有时只显示标记。来源：`~/.haa/usage.jsonl` 的 Task 记录（haa 改动之后才有 `sessionId`）与 `~/.haa/workers.json`（还在的 Worker）。

### token 与 Cost

agy 的 Session 显示 token 数；Cost 写「未记录」，不计入任何 Cost 合计，也不算「未定价」（不出现「+」）；统计页与它的下钻只算 Claude Code。

## 架构

做法是把 agy 的 step 译成现有的 `RawEntry` 记录，后面的组装、⌘F、CLI 沿用；改动只在边上。

### `crates/transcript/src/agy.rs`

整个仓库只有这个文件知道 agy 的原始格式（同 `code_mode.rs` 的规矩），模块注释列出每个入口给谁用。

- 位置：`brain/<id>/.system_generated/logs/transcript_full.jsonl`、`cache/conversation_metadata.json`、`presence/<id>.lock`。
- 解析：每行一个 step，字段全部可选，坏行计数（同 `parse.rs`）；同一 `step_index` 出现多次时以最后一行为准。
- 翻译 `adapt(conv_id, steps) -> Vec<EntryRecord>`，排成一条直链（每条的 `parentUuid` 是上一条）：

  | step | 译成 |
  |---|---|
  | `USER_INPUT` | 用户消息：只取 `<USER_REQUEST>` 与 `</USER_REQUEST>` 之间的文字，后面的 `<ADDITIONAL_METADATA>`、`<USER_SETTINGS_CHANGE>` 丢掉；没有这个标签时取整段 |
  | `PLANNER_RESPONSE` | assistant 消息：模型、token（input / output / cache read）、思考、正文、`tool_use` |
  | `GENERIC` | `tool_result`，按顺序对上前一条回复的第几个调用；去掉开头的 `Created At` / `Completed At`；状态 `ERROR` 为出错。`RUNNING` 也是结果：命令转入后台时的那段说明（task id、命令），之后的系统消息会说它结束了 |
  | `SYSTEM_MESSAGE` | 系统条目，去掉 `<SYSTEM_MESSAGE>` 包裹与它前面那句说明 |
  | `ERROR_MESSAGE` | 错误级别的系统条目 |
  | 其他 | Unknown |

  id 由 conversation id 与 step 下标（工具再加调用序号）确定地生成，重读不变，也不会和 Claude Code 的撞。时间取 `created_at`。标题、cwd 不写进记录：它们是 Session 级的输入（见下），metadata 变了不用重新翻译。

- 工具：

  | agy | 译成 | 参数 |
  |---|---|---|
  | `run_command` | Bash | `CommandLine` → command，`toolSummary` → description |
  | `read_file` | Read | `Path` → file_path，`StartLine` → offset，`EndLine − StartLine + 1` → limit |
  | `edit_file` | Edit | `Path` → file_path，`Target` → old_string，`Replacement` → new_string |
  | `write_file` | Write | `Path` → file_path，`Content` → content |
  | `update_steps` | TodoWrite | `steps[].title` → content，`status` 原样（认不出的当 pending） |
  | `search_web` | WebSearch | `query` |
  | 其他（`mcp_*`、`manage_task`、`invoke_subagent`、`schedule`、`send_message` …） | 原名 | 原样，通用视图 |

  译名对应的必需参数缺了就不译，按原名走通用视图。原名与原参数放进调用的附加信息，给工具详情用。`mcp_` 开头的不译：服务名与工具名之间也是 `_`，拆不开。
- 诊断：agy 已知的 step 类型与工具名（按原名，`mcp_` 开头的算认得）也列在这里；翻译时按它判定，认不出的才计数。

### codeck-core

- 数据根：`EngineConfig` 加 `agy_root`（`paths.rs` 解析，固定 `~/.gemini/antigravity-cli`；目录不存在也照样记下这个路径）。索引的 `meta` 与 `data_root` 一并记它：可写打开时不一致就删库重建（重建后照样写回），只读打开（CLI）时不一致就报错、不动索引（ADR-0013）。
- 扫描：另走 `brain/*/.system_generated/logs/transcript_full.jsonl`，Session ID 取目录名；按 inode 去重照旧。扫描结果带上文件的种类，`files` 表记下它。
- 读文件只有一个入口：按种类分派到 Claude Code 的解析或 agy 的翻译。同步（`compute.rs`）、打开对话（`session_cache.rs`）、CLI 遇到索引里没有的 Session 时的回退都走它；agy 的文件在这里总是整份重读，不沿用旧解析往后接（同一下标可能被改写），所以对话开着时也立刻看到改写。agy 的 Session 没有 sidecar 与 Subagent 目录。
- 同步：agy 的文件一变就走现有的 Reparse（不走 Append），这个 Session 的行全部重写；文件都不大。
- 全文检索：agy 的文件不进。写入时它的全文进度直接记为完成，不进积压。
- metadata：`sessions` 里存每个 agy Session 用到的条目字段（`Title`、`WorkspaceURIs`）的指纹。每次同步读一次 `conversation_metadata.json`，指纹变了就重算这个 Session 的行（不动对话缓存）；比的是原始字段，不是算出来的标题与 haa 标记（标题没有时回落首条提示词，haa 标记还来自工具调用，都在重算时由 JSONL 与 metadata 一起得出）。启动时的全扫描也一样比，所以应用关着时改的也会更新。
- 索引：`sessions` 加 `agent` 与 `haa_worker`，`SCHEMA_VERSION` +1。agy 的消息行照常写（Session 的 token 由它加总），但定价跳过 agy：cost 留空，不计入「未定价」。统计与它的下钻只算 `agent = 'claude'`。
- Live：只探测一次，结果给列表、详情、轮询、`is_live` 与 CLI 的 `wait` 共用。macOS 用 libproc 列出 `antigravity` 进程打开的文件，Linux 读 `/proc/<pid>/fd`；都只读、不加锁。`presence/<id>.lock` 对上的就是 Live，状态直接是 `Unknown`（不经 `LiveStatus::from_raw`，它会把没有状态当成空闲）。
- haa：读列表与详情时按 `sessionId` 查 `~/.haa/usage.jsonl` 与 `workers.json`（按 mtime 缓存，只读），得到 Main Session、Worker 名、Task 标题，像 `live` 一样附上，不进索引。
- 诊断：翻译除了记录还返回它按 agy 原名判定出的认不出计数，agy 的文件写入时用它代替按 Claude Code 名单算出的那份；读出时 Claude Code 的名单只过滤 Claude Code 的条目。界面上经 `diag_counts` 关联的文件与 Session 按 Agent 分开显示，不另建表。
- `resume_command` 按 Agent 给出。

### codeck-host

监听在 agy 数据根下单独映射：接受 `brain/<id>/.system_generated/logs/transcript_full.jsonl` 与 `cache/conversation_metadata.json`（现有的过滤会拒掉以 `.` 开头的目录）。`changed_sessions` 从 JSONL 的路径取 Session ID；批次里有 metadata 时 agy 这一侧整根比对（大小与 mtime 没变的文件不重读，只比指纹），不靠路径推 Session。Live 照旧每 3 秒轮询。应用信息里显示 agy 的数据目录。命令表不变，类型多了字段，重跑 `pnpm bindings`。

### 前端、iPhone、CLI

- `SessionSummary` 加 `agent` 与 `haaWorker`（Main Session ID、Worker 名、Task 标题，可缺）。列表标记、头部（token、「未记录」、「复制 Resume 命令」、不显示 Resume 按钮、haa Worker 的归属）按画布选定的样子做；工具详情显示 agy 的原调用。
- iPhone v1 只解码新增的可选字段，列表标记以后跟画布一起做。
- CLI：JSON 输出带 `agent`；`show` 认得索引里没有的 agy Session；`wait` 用共用的 Live（状态未知时只能判断还在不在）。

## 只读与出错

- ADR-0002 扩到 `~/.gemini/antigravity-cli` 与 `~/.haa`：只用 `File::open`；不打开 agy 的 SQLite；不对 presence 锁做任何操作；只写 CoDeck 自己的缓存与设置目录。
- 没有 agy 目录：没有 agy 的 Session，不报错，也不反复重建。
- metadata 没有、或没有这个会话的条目：标题用首条提示词，Project 落到 agy 数据目录。
- 坏行、认不出的 step、认不出的工具：与 Claude Code 的同等处理（计数、Unknown、通用视图），不丢内容。
- 结果比调用少（会话结束或还在跑）：没对上的调用显示为进行中。
- haa 的文件没有或读不懂：没有归属链接，haa Worker 标记照旧（它来自会话本身）。
- 进程探测失败：没有 agy 的 Live，记日志，历史不受影响。
- presence 锁先于 JSONL 出现（刚开的会话）：等文件出现再算。
- 用户自己在提示词开头写了 `<system-reminder>` 这类标签：会和 Claude Code 一样被当成非人工输入折叠，不处理。
- agy 改版：多出的字段忽略，新的 step 类型与工具走 Unknown 与通用视图；历史不依赖 language server。

## 设计画布

动界面之前，在「CoDeck 设计」画布上建「方案 · agy Session」页，用真实数据画 2–3 个可编辑画板：列表里的标记、haa Worker 的标记与归属、头部（token、「未记录」、「复制 Resume 命令」）。选定后再实现，实现后把采用的画板挪进对应区域页、删掉方案页。

## 文档

CONTEXT.md 加 Agent，Session、Live Session 的定义扩到 agy；`docs/spec.md` 加 agy 的 Session、对话记录、Live、haa Worker、Cost 的口径，并写明 agy 不进全文检索；README；根目录 CLAUDE.md 的数据流写两个数据根；`crates/transcript/CLAUDE.md` 写 `agy.rs` 是唯一知道格式的地方。

## 测试

- fixture：`crates/codeck-core/tests/fixtures/agy/`，迷你 agy 数据根（`brain/…/transcript_full.jsonl`、`cache/conversation_metadata.json`、一个残留的 presence 锁），由 `examples/sanitize.rs` 扩展后从真实会话脱敏生成；README 表格与 `fixtures_sanity.rs` 钉住数字，敏感信息检查照旧。
- `agy.rs` 的单元测试：每种 step；`<USER_REQUEST>` 后面带 `<ADDITIONAL_METADATA>` / `<USER_SETTINGS_CHANGE>` 的取法；`RUNNING` 的结果；每个译名工具（含 `Read` 的 limit）；同下标以后一行为准；按顺序配对（含末尾缺结果）；认不出的类型。
- Engine 集成测试：
  - Claude Code 与 agy 两个数据根一起建索引，列表带 `agent`；agy 的文件不进全文索引：用一个只在回答正文里出现的词，写入后（跑全文积压之前）它的全文进度已是完成，⌘K 没有命中、⌘F 能定位到。
  - 重开 Engine 后打开 agy 的对话；对话开着时 agy 改写了某个 step，再打开看到新内容。
  - 只改 metadata 的标题，列表跟着变；停掉 Engine 改了再开，也跟着变；同一批里既有 A 的 JSONL 又有 B 的标题变化，两个都更新；什么都没变的第二次同步不重算任何 Session。
  - 全量 rebill / 重新定价之后 agy 的 token 还在，请求数、未定价数与 Cost 都不因 agy 增加；统计不含 agy。
  - Resume 命令；haa Worker 标记与归属（fixture 里放 `~/.haa` 的文件）；Live（「哪个进程开着哪个锁」做成可注入的，测试里注入；进程探测本身另用本进程开着的文件测一次）。
  - CLI：agy 根不一致时只读打开报错、索引不动；`show` 一个索引里没有的 agy Session。
- 真实数据探针（`#[ignore]`，只读）：`~/.gemini/antigravity-cli` 全部可解析、认不出的数量、配对率（只输出，不作为通过条件）。
- 前端：mock 加几个 agy 的 Session（含一个 haa Worker），E2E 覆盖列表标记、对话记录（agy 的命令、读文件、编辑显示成 Bash / Read / Edit，工具详情里有原调用）、头部（token、「未记录」、「复制 Resume 命令」、没有 Resume 按钮）。
- 实机：`pnpm tauri dev` 读真实数据，agy 的 Session 出现、开着的 agy 有 Live 标记。

## 实施顺序

小步提交：

1. `agy.rs` 与它的单元测试。
2. 扫描、读文件入口、索引、Engine（含 fixture 与集成测试）。
3. Live 与 haa。
4. Host 的类型、监听与 bindings、CLI。
5. 设计画布，选定后做前端与 E2E。
6. 文档。

haa 的 `sessionId` 改动在 PaaA 仓库单独做。
