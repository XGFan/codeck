# haa 的 codex / pi Worker：只在面板里按 id 读它们自己的记录

要的只是在派活的 Session 里看 haa 派出去的 Worker 做了什么：状态条那一行、Task 卡片点开右侧面板。agy 的 Worker 早已是 Session（ADR-0018），codex、pi 的还看不到。haa 的 `ls` 与 `usage.jsonl` 的 Task 记录都带后端的 `sessionId`（codex 的 thread id、pi 的 session id），和两边自己记录文件的 id 一致。

- **读后端自己的原始记录**：codex 的 rollout（`~/.codex/sessions/…/rollout-<时间>-<thread id>.jsonl`，旧的是 `.jsonl.zst`）、pi 的 session 文件（`~/.pi/agent/sessions/<目录>/<时间>_<id>.jsonl`），只读。格式只有 `crates/transcript/src/codex.rs`、`pi.rs` 知道，译成 Claude Code 的条目后照常组装（同 ADR-0018 的做法）。
- **不是 Session**：不扫描、不进索引、不进列表、全文检索与统计；索引里找不到一个 id 时，Engine 才按 id 去两边的目录找文件。对话、工具详情、图片因此都走原来的命令，前端只多一个判断：haa 模型名的后端是 `codex` 或 `pi` 的 Worker 有 `sessionId` 就能打开。
- **codex 只取它界面显示的那一份**（`item_completed` 的消息、命令、文件改动、MCP 调用）：模型那一份（`response_item`）里是开发者指令和调用工具的 `exec` 脚本，读起来不像 codex 自己显示的过程。

## Considered Options

- **读 haa 自己的 Worker Transcript**（`~/.haa/transcripts/<worker id>.jsonl`）：三个后端一个格式，最省。但它是给 `haa tail` 的精简副本（codex 的推理不记，工具结果只是一段字符串），`haa stop` 时就删掉；要的是原始记录。
- **当成 Session 进索引**（像 agy）：能进侧栏、搜索、单独打开，但要扫描与同步两个目录、加 Agent 种类、改列表与统计的口径；本机 pi 一半、codex 过半的记录是 haa 的测试与冒烟在临时目录里起的，全收会多出一批「目录已不存在」的 Project。只为在面板里看，不值得。

## Consequences

- codex、pi 的 Worker 对话按内容搜不到，只能从派活的 Session 打开；面板里没有「单独打开」，指标取 haa 的数据（Task 个数与时长）。
- 正在干活的 Worker 不经文件监听：面板每 2 秒重读一次，每次整份重读、重新组装（codex 一次评审的 rollout 有几 MB）。
- haa 写下 `sessionId` 之前的旧 Task 记录打不开（codex 的在 2026-11-27 之前，pi 的全部）。
- codex 把一个 thread 接着写进另一个文件（`…-<thread id>_<id>.jsonl`）时只读第一个文件；本机只有 3 个，都早于 haa。
