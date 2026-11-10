---
name: codeck
description: 查询本机 Claude Code 的历史 Session（列出、全文搜索、阅读对话）、等一个运行中的 Session 跑完或停下，或新建一个 Session 交给它干活。Use when the user asks what was done or said in an earlier Claude Code session, wants to find a past conversation, or wants to hand work off to a new Session they can take over later.
---

# codeck

用 `codeck` 查这台机器上的 Session。除 `new` 外全部只读。不需要 CoDeck 桌面应用开着，但数据来自它建的索引：桌面没开时不更新，所以结果里带 `indexedAt`，新近的 Session 可能还查不到。

每个命令的 stdout 是一个 JSON 文档（`show` 是文本）。失败时退出码 1，stderr 是一行 `{"code","message"}`。

## 找一段过去的对话

```sh
codeck sessions --project "$PWD" --limit 10          # 最近的 Session，带 Live 状态
codeck sessions --live                               # 现在在跑的
codeck search '"短语" 词 -排除词' --project "$PWD"     # 全文搜索，不搜工具输出
codeck show <sessionId>                              # 每轮一段的概览
codeck show <sessionId> --turn 7                     # 这一轮的完整过程与回答
codeck show <sessionId> --turn 7 --full              # 再加上工具的输入和输出
codeck show <sessionId> --agent <agentId>            # 一个 Subagent Run
```

`--project` 收 Project 的路径（仓库根目录，不是子目录）。

先 `search`，再用命中里的编号 `show --turn`：

- `location` 是 `mainLine`：`turn` 就是 `show --turn` 的编号。
- `location` 是 `subagent`：`turn` 为 null，用 `show <id> --agent <agentId>` 阅读。
- `location` 是 `branch`：命中在用户回退掉的 Branch 上，已不在对话里，`show` 看不到。
- `location` 是 `unknown`：文件在建索引之后变了，这条命中找不到位置。

`show` 的概览里过长的文字会截断并标明，工具调用只列动作和对象；要看全文用 `--turn`，要看工具的输入和输出再加 `--full`。头部带 `partial` 表示索引里还没有这个 Session，内容是直接读文件的，没有 Fork 与接续信息。

## 等一个运行中的 Session

```sh
codeck wait <sessionId> --timeout 110
```

Session 变成 idle、waiting 或结束时立即返回 `{"sessionId","status","waitingFor"}`。超时退出码 124，stdout 仍是当前状态，再调用一次即可。默认 110 秒，留在 Bash 工具默认的 120 秒以内。id 不存在时报 `not_found`。

## 新建 Session 交给它干活

```sh
codeck new --cwd /path/to/project <<'PROMPT'
把要它做的事写在这里。
PROMPT
```

提示词从 stdin 读，成功时输出 `{"sessionId","holderId","cwd"}`，Session 在 CLI 退出后继续运行。

- 提示词要自成一体：新 Session 看不到你的上下文，要写清目标、相关文件和完成标准。
- 提示词不能以 `!` 开头，否则会被拒绝：TUI 会把它当 shell 命令直接执行。
- 发出首条提示词后 CLI 就放手。Session 碰到审批或提问时停在终端等人，`wait` 返回 `waiting`；不要替用户作答，告诉用户它在等什么。
- 目录没有被信任过时 `new` 失败（`untrusted_dir`），这是安全边界：不要绕过，请用户先在终端里用 claude 打开一次这个目录。
- `new` 启动失败且 Pane 还在时，错误里带 `holderId`，告诉用户去那个 Pane 接手。
