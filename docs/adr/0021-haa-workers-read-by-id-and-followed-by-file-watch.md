# haa Worker 的对话：agy、codex、pi 一样按 id 现读，看文件变化实时跟随

接入其他 Agent 只为一件事：在派活的 Session 里看 haa 派出的 Worker 做了什么、做到了哪一步。agy 原先按完整接入做（ADR-0018 的历史部分：扫描、索引、侧栏、搜索、Live、整页打开），codex、pi 只在 Worker 面板里按 id 现读（ADR-0020）。现在三者拉平，都照 ADR-0020 的做法。

- **agy 的对话也不是 Session**：不扫描 `brain/`、不读 `conversation_metadata.json`、不进索引、列表、侧栏、搜索与统计，不看 presence 锁，没有整页视图与「复制 Resume 命令」；不是 haa 起的 agy 对话不再显示。`agy.rs` 的译法不变，Engine 在索引里找不到一个 id 时按 id 找 `brain/<id>/.system_generated/logs/transcript_full.jsonl`，与 codex、pi 并列。
- **入口只有派活 Session 里的两处**：haa 状态条那一行与 Task 卡片，打开右侧面板。三个后端的 Worker 有 `sessionId` 就能打开；面板的指标行都取 haa 的（`<Agent> · 模型`、Task 个数与 Task 时长），没有「单独打开」。
- **实时跟随看文件，不 Connect**：Host 监听三个后端的会话目录（agy 的 `brain/`，codex 的 `sessions/` 与 `archived_sessions/`，pi 的 `sessions/`），从变化的路径认出对话 id，发事件；打开着这个对话的面板随之重读，取代原来的每 2 秒重读。能多快看到一步，取决于后端什么时候写文件：agy 每个 step 都写（没跑完的命令先写成 `RUNNING`，之后追加一行覆盖），pi 每条消息结束时写，codex 只写已完成的 item（长命令跑完才出现）。Worker 运行中时，面板底部照 haa `ls` 的 `activity` 写「正在做：…」，补上这段空档。

## Considered Options

- **保留 agy 的完整接入，把 codex、pi 也提上来**（扫描、索引、侧栏）：ADR-0020 已否决（两边一半以上的记录是 haa 的测试在临时目录里起的，还要改列表与统计的口径）；三者的 Live 也没法统一判断（agy 看 presence 锁，codex、pi 没有可看的）。
- **逐字流式**：由 haa 守护进程把它从后端收到的增量（ACP 的消息块、codex app-server 的 delta、pi 的 `message_update`）转给 CoDeck，面板末尾显示一段临时内容。要两个仓库一起改，还要在原始文件写到时与临时内容对上；先不做，「正在做」那一行不够用时再议。
- **只在 Worker 运行中轮询，或只监听打开着的那个文件**：监听整个目录更简单，后端没在跑时这些目录里没有变化。

## Consequences

- ADR-0018 的历史部分由本 ADR 取代；它的 Connect 部分（接 agy 的 language server）没有实现，搁置。CONTEXT 里 Session 只有 Claude Code 的，Agent 不再是 Session 的属性。
- 本机 `brain/` 下 216 个 agy 对话不再出现在列表里（含人手动开的）；旧的 agy Task 记录已由 haa 的回填脚本补上 `sessionId`，照样能从 Task 卡片打开。
- agy 的 Worker 对话按内容搜不到，认不出的条目照原文显示、不再进诊断，与 codex、pi 一样。
- 每次变化仍整份重读、重新组装；codex 一次评审的 rollout 有几 MB，实测慢了再改成增量读（三个文件都只追加）。
- 索引的 schema 变了（去掉 Agent 与 agy 的列），按 ADR-0003 重建。
