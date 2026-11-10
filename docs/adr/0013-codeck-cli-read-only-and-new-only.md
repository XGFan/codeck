# codeck CLI：只读加只新建，直接读索引，不走 daemon，不做 MCP

要让 AI（典型是 Claude Code 经 Bash）能查询本机的 Session，并交出一个新任务。我们加了一个 `codeck` 可执行文件（`crates/codeck-cli`）：`sessions`、`search`、`show`、`wait` 只读，唯一的写是 `new`（经 ccrpc 新建 Session、发出首条提示词后放手）。向已有 Session 追加提示词、作答审批与提问、Resume、Fork、改设置一律不做：这些要么需要 Connect，要么是替人做决定，审批停在 TUI 等人是这里要保住的安全边界。目录信任框同理：`new` 答「不信任」让这个 claude 退出，报错请人先在终端里打开一次。

`sessions`、`search`、`show` 直接打开桌面（或 daemon）建好的索引，不启动 `Host`，也不需要应用开着。索引可能滞后，所以结果带 `indexedAt`，`show` 的文件内容总是读最新的，索引没收录的 Session 退回直接读文件并标 `partial`。这要求一种新的打开方式：`Index::open_read_only` 不建目录、不建库、不写 `user_version`，版本或数据根不一致时报错；否则版本不同的 CLI 一运行就会把桌面的索引删掉重建（`Index::open` 的行为，ADR-0003）。SQLite 对 WAL 库的只读连接仍会在旁边建 `-wal` / `-shm`，所以没有 `-wal` 时（没有写入者开着索引）用 `immutable` 打开，什么文件都不留下；这时若桌面刚好启动并写入，这一次读可能不一致，但读的是一次性的命令，下一次就对了，换来的是 CLI 永远不改动索引目录。

`show --turn` 的编号和 `search` 命中给的编号是同一套：对话第 N 条用户输入（Human / Command，Subagent Run 里还有 Coordinator）开始第 N 轮，由同一个函数在带隐藏节点的 Transcript 上算出。命中经 `resolve_jump` 换成显示节点再查轮次，这样落在 Subagent Run 或已回退的 Branch 上的命中如实写 `turn: null` 与位置，不给错误的数字。

## Considered Options

- **经 daemon 的 HTTP**：现成的 `POST /v1/rpc` 也能查，但要 daemon 在跑，桌面用户没有；还要处理局域网暴露与 Connect 独占。CLI 只读索引，不抢 Connect，不需要常驻进程。
- **MCP server**：要 AI 的配置里多一项，常驻占上下文；一个带 `--help` 的可执行文件加一份可选的 SKILL.md 就够了，也符合 ccrpc 不改变 Claude Code 表现的原则（不进插件，不加 MCP）。CLI 不写数据根，SKILL.md 由用户自己放进 `~/.claude/skills/`。
- **把 IPC 类型直接作为 JSON 输出**：省一层转换，但 IPC 类型跟着界面变化，AI 脚本会跟着坏；输出类型在 `codeck-cli` 里单独定义，只给 AI 需要的字段，并限长。
- **`new` 把信任框也答掉**：省一步，但信任一个目录是人的决定，模型不能替人同意。

## Consequences

- CLI 依赖索引存在且版本一致；索引不存在或版本不同时报错退出，不做修复。CLI 与应用的 `SCHEMA_VERSION` 一起发布。
- `sessions`、`search` 看不到还没进索引的 Session；`sessions --live` 也只列索引里有的（Live 状态本身是实时的）。
- `new` 没有 ccrpc 连着之后，hooks 不起作用，审批停在 TUI；`wait` 从进程文件读到 `waiting`，桌面可以照常 Connect 接手。
- 桌面崩溃后留下 `-wal` 而没有 `-shm` 时，只读读取仍会建出 `-shm`；数据库与 WAL 的内容不变。
- 有人在 Pane 里把对话转到后台或 `/clear` 后，进程文件不再对应原来的 Session，`wait` 可能超时，或报 `idle`、`ended`。
