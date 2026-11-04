# CLAUDE.md

cc-gui：基于 ccrpc 的 Claude Code 桌面 GUI（Tauri 2 + React 19），只在本机用，用来在日常开发中验证 ccrpc。规格见 [docs/spec.md](docs/spec.md)，关键取舍见 [docs/adr/](docs/adr/)，术语沿用 [ccrpc 的 CONTEXT.md](../../crates/ccrpc/CONTEXT.md)。

## 硬约束

- 内容只来自 ccrpc 协议（[ADR-0001](docs/adr/0001-content-only-from-ccrpc.md)）：不读 transcript、进程文件，不引入 cv-core。缺什么补进 ccrpc。
- 对认不出的界面（`dialog` / `unrecognized`）不按键，只显示屏幕原文。

## 排查时看哪里

用户遇到问题会另开一个终端 Session 来排查。日志在 `~/Library/Logs/cc-gui/<日期>/<holder_id>-<启动时间>/`，最新的就是按目录名排序的最后一个；`wire.jsonl` 第一行的 `ready` 里有 holder_id、session_id、cwd、claude 版本。三层按时间戳对齐：

- `wire.jsonl`：GUI 与 ccrpc 之间的原样往返。先看这里分清归属——ccrpc 发出的事件就不对 → ccrpc 的问题（到 `crates/ccrpc` 修）；事件对而显示不对 → GUI 的问题。
- `ccrpc.log`：ccrpc 为什么这么判断（状态判定、识别决策、hook 事件、按键、出错时的整屏）。
- `gui.log`：用户点了什么、对应哪条命令 id、渲染异常、ccrpc 的退出码与 stderr。

GUI 的问题修好后，把相关的 wire.jsonl 片段（脱敏）补成 E2E 回放 fixture。
