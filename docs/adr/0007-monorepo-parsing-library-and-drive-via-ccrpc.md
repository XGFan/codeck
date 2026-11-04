# 一个仓库：解析是库，控制经 ccrpc；cc-gui 并入 Viewer

Claude Viewer、ccrpc、cc-gui 原是三个仓库，各自实现了一遍「把 transcript 变成可显示的内容」：cv-core 的 assemble、ccrpc 的 transcript 尾随、cc-gui 前端把 ccrpc 的 block 拼成消息行。Claude Code 的格式一变要改三处，三处的行为也已出现分歧（`<pasted_content>` 标签、Bash 输出细节）。ccrpc 依赖 cv-core 的内部模块，却靠提交在仓库里的 `[patch]` 指向同级目录，单独构建不了；cv-core 里有一部分改动只是为了给 ccrpc 用。

因此合成一个仓库（一个 Cargo workspace），功能按两块分：

- **transcript 解析是库**（`crates/transcript`）：容错解析、组装成显示模型、进程文件、价格。进程内调用——Viewer 建索引要批量解析成千上万个文件，走进程通信没有意义。cv-core 只剩索引、搜索、统计；ccrpc 只依赖这个库，不再编译 SQLite。同一条判定规则只在库里实现一次。
- **Claude Code 的控制经 ccrpc 进程**（JSONL 协议）：ccrpc 持有 PTY、hook socket 与状态机，是独立的产品，脚本照样可以用它。

cc-gui 原本的意义是在日常使用里验证 ccrpc（为此它的内容只来自 ccrpc、不共享 Viewer 的代码）；Viewer 能驾驶 Session 之后它只是 Viewer 的裁剪，所以把驾驶功能（输入框、审批与提问卡片、后台任务、状态与权限模式、三层日志）移植进 Viewer，删除 cc-gui。

Viewer 驾驶一个 Session 时，**对话内容仍来自它自己的索引与实时跟随，ccrpc 只提供状态与控制**。这样 Viewer 前端只有一条内容路径；审批卡片自带工具输入，不依赖对话里已经显示出那次工具调用。只能驾驶 ToH clay 里装了 ccrpc 插件的 Session，其他 Live Session 仍只读跟随。

## Considered Options

- **保持三个仓库，只把 `[patch]` 移到不提交的本地配置**：改动最小，但跨仓库改动仍要协调提交，三份解析与渲染照旧。
- **Viewer 驾驶时内容也走 ccrpc 的内容流**：状态与内容天然同序，但 Viewer 前端要多一条完全不同的内容数据路径（翻页、Branch、Subagent Run 面板都要重做一遍），而它已有的实时跟随足够及时。
- **ccrpc 的协议改成规范显示模型、从 Rust 类型生成 TS**：ccrpc 用 `json!` 构造协议，重写协议层的收益只在 cc-gui，而 cc-gui 被并入后已无前端消费 ccrpc 的内容流。
- **保留 cc-gui 作为 ccrpc 的参考客户端**：Viewer 驾驶时同样经 ccrpc 的协议，验证作用由 Viewer 承担；维护两个前端不划算。

## Consequences

- 取代 cc-gui 的 ADR-0001（内容只来自 ccrpc、渲染组件复制而不共享）。
- 应用、仓库、bundle id 与 `cv-` 前缀的改名留待之后（「Viewer」已不贴切）。
- Viewer 不再只是只读的查看器：驾驶时会经 ccrpc 向 Claude Code 写入（PTY 与插件回复）；对数据目录的只读保证（ADR-0002）不变。
