# 开发流程

## 分工

| 角色 | 负责 |
|---|---|
| Main（Claude Code） | 实测 Claude Code 行为、屏幕识别、状态机、协议定稿、拆 Task、审查与验收、提交 |
| pi Worker（经 paaa） | 规格明确的模块（如 `rpc.rs`）、样板、补测试、按 spec 写文档 |
| 独立审查（code-reviewer 子代理） | 每个里程碑提交后的代码审查；写的人不自己批准 |
| 用户 | 方向与取舍、提供端到端测试用的模型 key |

给 Worker 的 Task 必须写明：只允许改哪些文件、不许对整个仓库跑 `cargo fmt`、不许 `git checkout` / `restore` / `commit`（曾因此丢过 Main 未提交的改动）。

## 里程碑

| | 内容 | 状态 |
|---|---|---|
| M0 | 实测探针：进程文件、transcript、写入、审批框、AskUserQuestion、steer、/clear、Subagent；录 fixture（[probes.md](./probes.md)） | 完成 |
| M1 | 最小闭环：`new` / `attach` + `prompt` + `block` / `state` / `turn_end` | 完成 |
| M2 | 审批与 AskUserQuestion 作答 | 完成 |
| M3 | `steer` / `follow_up` / `abort` / 斜杠命令 / `send_keys` / `dialog` | 完成 |
| M4 | Subagent Run 事件 | 完成 |

## 验证

每次改动：`cargo test`、`cargo clippy --all-targets -- -D warnings`。改到 `session.rs`、`recognize.rs`、`pty.rs`、`transcript.rs`，或升级 Claude Code 后，再跑一次端到端（见 [README](../README.md#测试)）。Claude Code 升级导致识别失败时：用 `ccrpc debug screen` 拍下新布局存进 `tests/fixtures/screens/`，改识别器，更新 probes.md 里的版本与布局。
