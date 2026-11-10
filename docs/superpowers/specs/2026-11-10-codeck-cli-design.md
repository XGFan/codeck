# codeck CLI 设计

让 AI（典型是 Claude Code 经 Bash）查询本机的 Session，并新建 Session。除 `new` 外全部只读。

## 目标与边界

- 查询：列出 Session、全文搜索、阅读对话、等 Live Session 跑完或停下。
- 唯一的写：`new`，在 clay 里新建一个 Session 并发出首条提示词，然后放手。
- 不做：向已有 Session 追加提示词、作答审批与提问、Resume、Fork、放宽权限、MCP server、JSON Schema 输出、改设置。
- 不需要 daemon，也不需要桌面应用开着。
- 对数据根严格只读（ADR-0002）：CLI 不写 `~/.claude` 下任何文件。

## 命令

新增 crate `crates/codeck-cli`，可执行文件 `codeck`。依赖 `codeck-core`、`transcript`，以及 `codeck-host` 的设置与启动规则（不启动 `Host`）。

| 命令 | 作用 | 数据来源 |
|---|---|---|
| `codeck sessions [--project P] [--live] [--limit N]` | 列出 Session，带 Live 状态 | 索引 + 进程文件 |
| `codeck search "词" [--project P] [--limit N]` | 全文搜索，语法同 ⌘K（`"短语"`、`-排除词`），命中带 sessionId 与轮次 | 索引 |
| `codeck show <id> [--turn N] [--full] [--agent ID]` | 紧凑文本形式的对话 | 见下 |
| `codeck wait <id> [--timeout 秒]` | 阻塞到 Session 变成 idle、waiting 或结束 | 进程文件 |
| `codeck new --cwd DIR` | 新建 Session，提示词从 stdin 读 | ccrpc |

`--project P` 收 Project 的路径，CLI 查索引换成 Project ID。

全局选项 `--cache-dir DIR`：索引所在目录，默认桌面的 `~/Library/Caches/dev.joy.codeck/`。daemon 的 Host 上指向 `<状态目录>/cache`。

数据根的确定顺序与桌面相同：设置中的覆盖 → `CLAUDE_CONFIG_DIR` → `~/.claude`。

## 输出

- 除 `show` 外，stdout 只有一个 JSON 文档。所有基于索引的结果带 `indexedAt`，让调用方知道数据有多新。
- 成功退出码 0；`wait` 超时退出码 124，stdout 仍是当前状态；其他错误退出码 1，stderr 是一行 JSON `{"code","message"}`，`message` 是面向人的一句中文。
- 输出类型在 `codeck-cli` 里单独定义，从 IPC 类型转换而来。IPC 类型跟着界面变化，不作为 AI 的接口。用法靠 `--help` 与 SKILL.md 说明。
- 输出要有上限：`--limit` 有默认值，文本字段按长度截断并标明。

`show` 默认只看 Main Line，每轮一段，形如：

```text
session 6f0e2b7c  status=idle  turns=2  project=/Users/me/proj

[1] 用户：给 src/util.rs 补一个单元测试
    工具：读取 src/util.rs；编辑 src/util.rs；运行 cargo test
    回答：已补上单元测试。

[2] 用户：…
```

`--turn N` 只输出第 N 轮，`--full` 附上该轮完整的工具输入与输出（经 `get_tool_detail`，带长度上限），`--agent ID` 切到一个 Subagent Run。轮次编号就是 `search` 命中里给的编号，保证搜索结果能直接接 `show --turn`。

## 只读打开索引

现状：`Index::open` 在 schema 版本或数据根不一致、或文件读不了时会删库重建，`Engine::open` 也会创建目录。版本不同的 CLI 直接用会把桌面的索引删掉。

新增 `Engine::open_read_only(cfg)`（底层 `Index::open_read_only`）：

- 所有连接都以只读方式打开，不创建目录，不建库，不写 `user_version`，也不注册写入用的函数。
- 索引不存在、schema 版本或数据根不一致时返回错误，不做任何修改。错误文案说明原因，例如「索引版本与 CLI 不一致，请升级 CoDeck」。
- 不运行同步、监听和价格下载。Live 状态直接读进程文件。

索引可能滞后，桌面不开时不更新，规则如下：

- `sessions`、`search` 查到的是索引内容，靠 `indexedAt` 告知新旧。`sessions --live` 的 Live 状态是实时的。
- `show`：Session 的文件列表、Fork 来源、compact 接续、Subagent 统计来自索引，文件内容总是读最新的（`session_cache` 按 path + size + mtime 校验）。索引没收录这个 Session 时（刚建的），退回直接定位 `projects/*/<id>.jsonl` 组装，此时没有 Fork 与接续信息，输出里标明 `partial`。
- 索引滞后时，已收录的 Session 新产生的 Subagent Run 可能看不到。
- `wait` 完全不用索引。

## `new` 的流程

1. 校验 `--cwd` 是已存在的目录、stdin 的提示词非空且不以 `!` 开头（ccrpc 会当 TUI 的 shell 命令执行，绕过审批）。读设置里的启动规则与「新开的 Pane」，用 `launch::args_for` 拼出 claude 参数，和桌面新建 Session 一致。
2. 启动 `ccrpc new [--hidden] --cwd DIR [-- 启动规则的参数]`。ccrpc 的查找顺序同桌面：`CODECK_CCRPC`、`~/.local/bin/ccrpc`、PATH。
3. 读 stdout，等到 `ready`，取 `holder_id`；`session_id` 常为 null，这时再等 `session_changed`。超时 60 秒。
4. 发 `{"type":"prompt","text":…}`，等对应的 `response` 成功。
5. 关闭 stdin，ccrpc 断开，Session 继续运行。输出 `{"sessionId","holderId","cwd"}`。

断开后没有 ccrpc 连着，hooks 什么也不做，审批停在 TUI 等人；`wait` 会返回 `waiting` 与等待原因。之后桌面可以照常 Connect 接手。

异常：

- 目录信任框（`ready` 之前的 `choice`，layout 为信任）：这是安全边界，AI 不代答允许。CLI 答「不信任」让这个 claude 退出，再关掉 Pane，不留下空 Session，报错「目录还没有信任，先在终端里用 claude 打开一次」。
- `ready` 之前的其他 `choice`、超时、提示词发送失败：关闭 stdin，报错并带上已知的 `holderId`，Pane 留给人处理。
- ccrpc 启动失败或提前退出（没装 ToH、没装插件等）：报错，一句话说明原因。完整输出不进文案。

## `wait`

每秒用 `transcript::live` 读进程文件，判断 `<id>` 的状态：

- `idle` / `waiting` / `ended`（进程文件连续两次读不到）时返回 `{"sessionId","status","waitingFor"}`。
- 既没有进程文件也没有 transcript 的 id 报 `not_found`。
- 默认超时 110 秒，低于 Bash 工具默认的 120 秒，超时后返回当前状态与退出码 124，调用方再次调用即可。

## Skill

SKILL.md 放在仓库 `crates/codeck-cli/SKILL.md`，内容是何时用这些命令、`search → show --turn` 的用法和 `new` 的边界（提示词要自成一体、停在审批时由人接手）。

用户自己决定是否安装，手动复制到 `~/.claude/skills/codeck/`，CLI 不写数据根。这也符合 ccrpc 不改变 Claude Code 表现的原则：不把它放进 ccrpc 插件，也不加 MCP。

## 测试

只写高价值的：

- `Index::open_read_only`：版本不一致、数据根不一致时报错，且文件的内容未被改动（防止回归成删库）。
- `show` 的渲染：用现有 fixture 校验轮次编号、截断、`search` 的轮次能接上 `show --turn`。
- `new` 的驱动：用假的 ccrpc 脚本（`CODECK_CCRPC`）覆盖正常流程、信任框、提前退出、超时。
- `wait`：伪造进程文件，覆盖 idle、waiting、结束与超时。

单测盖不到的，本地实测：真实的 `codeck new`，确认 Session 在 CLI 退出后继续运行、桌面能 Connect、审批时 `wait` 返回 waiting；信任框答「不信任」后 claude 确实退出。

## 要同步的文档

ADR-0012（CLI 的取舍：只读 + 只新建、不走 daemon、不做 MCP）、README、CLAUDE.md 的仓库布局与常用命令、`docs/spec.md`。

## 分期

1. 只读打开索引与只读命令：`sessions`、`search`、`show`。
2. `wait` 与 `new`。
3. SKILL.md 与文档。
