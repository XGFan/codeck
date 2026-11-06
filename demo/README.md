# 演示数据根

`root/` 是一个形同 `~/.claude` 的迷你数据根，专门用来演示 CoDeck：Project lumen-api 下 4 个 Session，覆盖对话记录里的各种消息类型（与设计画布「2 · 对话记录」各行对应）。

| Session | 内容 |
|---|---|
| 为 /v1/chat 接口增加令牌桶限流中间件 | 主线故事：各种提示词（代码、斜杠命令、`!` 命令、图片）、思考、Markdown 回答、各工具的成功 / 失败 / 后台、并行与后台 Subagent 及其结束通知、向你提问、compact、本地命令、API 错误与重试、Branch |
| 为 /v1/chat 接口增加令牌桶限流中间件 (Branch) | 从主线第一轮之后 Fork 出来 |
| Redis 计数与压测并行 | Teammate：派出、报告、完成、空闲、失败、分配任务、退出；另一个会话发来的消息 |
| 复核限流改动 | Workflow：两个阶段、一个 agent 失败 |

## 怎么看

设置里把数据根指到 `demo/root`，看完切回。或者不碰自己的设置与索引，用独立的 HOME 起开发版：

```sh
env HOME=/tmp/codeck-demo-home CARGO_HOME=~/.cargo RUSTUP_HOME=~/.rustup \
  CLAUDE_CONFIG_DIR=$PWD/demo/root pnpm tauri dev
```

## 怎么来的

不是手写的 JSONL，也不是某个真实 Session 改的：

- `shapes/`：形状库。每种条目一个本机真实样本（claude 2.1.318 为主，本版本没有的取最近的版本），经 `crates/codeck-core/examples/sanitize.rs` 脱敏、id 换成占位，作模板。所有字段都来自真实条目。
- `story/*.ts`：故事脚本，只写文字、顺序与选择（成功 / 失败 / 后台…）。
- `scripts/demo/`：生成器。按 Claude Code 的规则填 id 与链接：一条 assistant 消息按内容块拆成多条 entry、tool_result 挂在自己的 tool_use 上、后台任务的启动文字与结束通知、Subagent / Workflow 的 sidecar、Fork 的 `forkedFrom`。同样的输入生成逐字节相同的输出。

```sh
node scripts/demo/build.ts            # 改了 story 或生成器后重新生成 root/
node scripts/demo/extract-shapes.ts   # 换了 Claude Code 版本、要新的条目类型时更新 shapes/（只读 ~/.claude）
```

加新的消息类型：先确认 `shapes/` 里有它的真实样本（没有就把种类加进 `extract-shapes.ts` 的 `WANT` 再跑一次），再在 `scripts/demo/kit.ts` 写它的生成函数、在某个 story 里用上，最后把它加进 `crates/codeck-core/tests/demo.rs` 的 `REQUIRED`。

## 校验

`cargo test -p codeck-core --test demo`：用真实的 Engine 索引 `root/`，诊断里没有未知类型与认不出的消息；每个 Session（含 Subagent Run）都能组装，`REQUIRED` 列的类型都出现；系统注入的条目默认隐藏、其他 Agent 的消息默认显示；`demo/` 下没有 `/Users/dev` 以外的家目录与 `CODECK_FIXTURE_DENYLIST` 里的词。

已知与真实数据的差别：TodoWrite 与 Glob 在本机没有样本，没有放；Grep 的样本来自 2.1.114（新版本里 claude 用 Bash 搜索）；Subagent 都是后台的（2.1.172 之后 claude 只发后台 Subagent）。
