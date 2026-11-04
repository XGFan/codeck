# Project 以仓库为单位，Session 与 Project 多对多，用快速打开取代 Project 栏

原先 Project 是 Session 启动时 cwd 的 realpath，一个 Session 只属于一个 Project，界面是「Project 栏 | Session 列表 | 对话」三栏。但实际用法里常有几个仓库放在同一个工作区目录下（如 `~/Project/<公司>/<各个微服务>`）：排查问题或改动横跨多个服务时，在工作区根目录启动；明确要改两个仓库时，在其中一个启动再 `/add-dir` 另一个；Claude 也会自己 cd 过去或按绝对路径修改。实测本机 1133 个这类 Session 中，有 101 个改动了启动仓库以外的仓库；在工作区根目录启动的 199 个里，有 47 个改了 1–6 个仓库。启动 cwd 只能把它们归到工作区，或只归到其中一个仓库。

决定：

- **Project 是 git 仓库的根目录**，worktree（`.git` 文件指向 `<主仓库>/.git/worktrees/`）归入主仓库；不在仓库里的目录就是它自己。用户主目录不当作仓库根目录，否则主目录下的 dotfiles 仓库会吞掉所有目录。
- **Session 属于启动 Project，也属于每个改动过的 Project**。「改动」只看 Edit / Write / MultiEdit / NotebookEdit 的 `file_path`（含 Subagent），记在 `tool_calls.file_path`，每次同步后按 Session 重算 `session_projects`。
- **去掉 Project 栏**，只保留 Session 列表；用 ⌘P 快速打开按标题、Project 名、分支查找 Session，或选一个 Project 筛选列表。

不采用的做法：

- **用 `cwd` 序列或 Read/Grep 的路径算归属**：`cwd` 只反映 shell 停在哪里；读取几乎每个 Session 都会碰到公共目录（如 skills 仓库），噪声太大。
- **解析 `/add-dir`**：它只留下一段命令文本，启动参数 `--add-dir` 和设置里的 `additionalDirectories` 完全不留记录；而且加了目录不等于用了。
- **工作区作为单独的层级**（自动识别或手动配置）：多了一个需要判断或配置的概念，收益只是「看整个工作区」；按工作区启动的 Session 本来就以工作区为启动 Project。
- **双击 Shift**：macOS 中文输入法用 Shift 切换中英文，会频繁误触发。

## Consequences

- 判断依赖当前磁盘：仓库被删除或移走后，重建索引时改动不再计入它；已不存在的启动 cwd 归入仍存在的上级目录所在的仓库，没有时按原始路径归类。
- resume / Fork 复制进来的历史里的改动也计入新 Session，与「对话记录里能看到这些改动」一致。
- 用 Bash 做的修改（`sed -i`、代码生成等）不计入改动的 Project。
- 按 Project 筛选时，列表、搜索、统计都包含改动过它的 Session；统计的 Project 排行仍按启动 Project 归属，每个 Session 只计一次，因此排行里的 Session 数可能少于按该 Project 筛选出的数量。
- Project 身份变化需要重建索引（Schema 8）。

## 后续调整

- ⌘P 快速打开已并入 ⌘K 搜索，⌘P 不再使用。
- 仓库归属对「这次做了什么」并不准确，Project 改为只在搜索里出现，作为找 Session 的辅助（按 Project 查找、限定范围、筛选 Session 列表）；统计页的 Project 过滤和排行（改为 Session 排行）、Session 列表行上的 Project 名都已去掉。后端的归属计算保留。
