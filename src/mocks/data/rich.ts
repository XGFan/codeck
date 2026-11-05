import type { BranchOption, ImageRef, Node, SubagentRun, WorkflowRun } from "@/ipc/bindings";
import { asstNode, call, NOW, MIN, otherNode, text, thinking, tok, userNode } from "./builders";

/** The "rich" Session: branch point, compact, all tool kinds, subagents, workflow, hidden entries. */
export const RICH_ID = "7f3a9c10-0001-4000-8000-000000000001";

const START = NOW - 3 * 60 * MIN;
let t = START;
const ts = () => (t += 25_000);

const ANSI_ERR = [
  "\u001b[1m\u001b[38;5;9merror[E0432]\u001b[0m\u001b[1m: unresolved import `governor::clock::DefaultClock`\u001b[0m",
  " \u001b[1m\u001b[38;5;12m--> \u001b[0msrc/middleware/rate_limit.rs:3:5",
  " \u001b[1m\u001b[38;5;12m|\u001b[0m",
  "\u001b[1m\u001b[38;5;12m3 |\u001b[0m use governor::clock::DefaultClock;",
  " \u001b[1m\u001b[38;5;12m|\u001b[0m     \u001b[1m\u001b[38;5;9m^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\u001b[0m \u001b[1m\u001b[38;5;9mno `DefaultClock` in `clock`\u001b[0m",
  "",
  "\u001b[1m\u001b[38;5;9merror\u001b[0m\u001b[1m:\u001b[0m could not compile `lumen-api` (lib) due to 1 previous error",
].join("\n");

const WRITE_CONTENT = [
  "use std::sync::Arc;",
  "use governor::{Quota, RateLimiter};",
  "",
  "/// 按 API key 维度的令牌桶限流层。",
  "#[derive(Clone)]",
  "pub struct RateLimitLayer {",
  "    limiter: Arc<RateLimiter<String, dashmap::DashMap<String, governor::state::InMemoryState>, governor::clock::DefaultClock>>,",
  "}",
  "",
  "impl RateLimitLayer {",
  "    pub fn new(per_minute: u32) -> Self {",
  '        let quota = Quota::per_minute(per_minute.try_into().expect("per_minute > 0"));',
  "        Self { limiter: Arc::new(RateLimiter::keyed(quota)) }",
  "    }",
  "}",
].join("\n");

const PERSISTED_TEXT = [
  "<persisted-output>",
  "Output too large (180.0 KB). Full output saved to: /Users/dev/.claude/projects/-Users-dev-Developer-lumen-api/7f3a9c10-0001-4000-8000-000000000001/tool-results/bd2x9k1.txt",
  "",
  "Preview (first 2KB):",
  "running 412 tests",
  ...Array.from({ length: 10 }, (_, i) => `test middleware::auth::case_${String(i).padStart(4, "0")} ... ok`),
].join("\n");

/** Full text behind the persisted output. */
export const PERSISTED_FULL = [
  "running 412 tests",
  ...Array.from({ length: 412 }, (_, i) => `test middleware::auth::case_${String(i).padStart(4, "0")} ... ok`),
  "",
  "test result: ok. 412 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.41s",
].join("\n");

const screenshot: ImageRef = { nodeId: "n14", ordinal: 0, mediaType: "image/png", bytes: 68, toolUseId: "tu_img" };

const askQuestions = [
  {
    question: "限流超限时应返回什么？",
    header: "超限响应",
    multiSelect: false,
    options: [
      { label: "429 + Retry-After", description: "标准做法，客户端可据此退避" },
      { label: "503", description: "与过载保护共用状态码" },
    ],
  },
  {
    question: "限流计数存在哪里？",
    header: "存储",
    multiSelect: false,
    options: [
      { label: "进程内存", description: "单实例足够，重启后清零" },
      { label: "Redis", description: "多实例共享计数" },
    ],
  },
];

const clarifyQuestions = [
  { question: "按什么维度限流？", header: "维度", options: [{ label: "API key" }, { label: "IP" }] },
  { question: "默认额度是多少？", header: "额度", options: [{ label: "每分钟 120 次" }, { label: "每分钟 60 次" }] },
];

const askAnswers = {
  "限流超限时应返回什么？": "429 + Retry-After",
  "限流计数存在哪里？": "先放内存，留好换 Redis 的接口",
};

export const richMain: Node[] = [
  userNode(
    "n01",
    ts(),
    "给 lumen-api 的 /v1/chat 接口加一个基于令牌桶的限流中间件，按 API key 维度，默认每分钟 120 次。",
  ),
  otherNode(
    "n02",
    ts(),
    {
      kind: "attachment",
      attachmentType: "skill_listing",
      text: "可用技能：review、simplify、security-review（共 3 项）",
    },
    { hidden: true },
  ),
  otherNode(
    "n03",
    ts(),
    { kind: "attachment", attachmentType: "todo_reminder", text: "提醒：当前没有进行中的任务列表。" },
    { hidden: true },
  ),
  asstNode("n04", ts(), [
    thinking("\n需要先看现有中间件的注册方式，\n再决定是 tower Layer 还是 axum from_fn。\n\n"),
    text("先看一下现有中间件的组织方式。"),
    call(
      "tu_read1",
      "Read",
      { file_path: "/Users/dev/Developer/lumen-api/src/middleware/mod.rs" },
      {
        text: "     1\tpub mod auth;\n     2\tpub mod trace;\n     3\t\n     4\tpub fn stack() -> tower::ServiceBuilder<...> {\n     5\t    ServiceBuilder::new().layer(trace::layer()).layer(auth::layer())\n     6\t}",
      },
    ),
    call(
      "tu_bash1",
      "Bash",
      { command: 'rg -n "RateLimit|governor" --type rust', description: "查找现有限流相关代码" },
      {
        text: 'Cargo.toml:41:governor = "0.8"',
        extraJson: JSON.stringify({ stdout: 'Cargo.toml:41:governor = "0.8"', stderr: "", interrupted: false }),
      },
    ),
  ]),
  // Branch head (main line). The abandoned alternative is `richAlt`.
  userNode("n05", ts(), "先只加中间件，不要动 handler。"),
  asstNode("n06", ts(), [
    text("好的，新增 `rate_limit.rs` 并在 `stack()` 里注册。"),
    call(
      "tu_write1",
      "Write",
      { file_path: "/Users/dev/Developer/lumen-api/src/middleware/rate_limit.rs", content: WRITE_CONTENT },
      {
        text: "File created successfully at: /Users/dev/Developer/lumen-api/src/middleware/rate_limit.rs",
      },
    ),
    call(
      "tu_edit1",
      "Edit",
      {
        file_path: "/Users/dev/Developer/lumen-api/src/middleware/mod.rs",
        old_string: "ServiceBuilder::new().layer(trace::layer()).layer(auth::layer())",
        new_string: "ServiceBuilder::new().layer(trace::layer()).layer(auth::layer()).layer(rate_limit::layer())",
      },
      {
        text: "The file /Users/dev/Developer/lumen-api/src/middleware/mod.rs has been updated.",
        extraJson: JSON.stringify({
          structuredPatch: [
            {
              oldStart: 4,
              oldLines: 3,
              newStart: 4,
              newLines: 3,
              lines: [
                " pub fn stack() -> tower::ServiceBuilder<...> {",
                "-    ServiceBuilder::new().layer(trace::layer()).layer(auth::layer())",
                "+    ServiceBuilder::new().layer(trace::layer()).layer(auth::layer()).layer(rate_limit::layer())",
                " }",
              ],
            },
          ],
        }),
      },
    ),
  ]),
  asstNode("n07", ts(), [
    call(
      "tu_bash_fail",
      "Bash",
      { command: "cargo build", description: "编译检查" },
      {
        isError: true,
        text: ANSI_ERR,
        extraJson: JSON.stringify({
          stdout: "",
          stderr: ANSI_ERR,
          interrupted: false,
          returnCodeInterpretation: "exit code 101",
        }),
      },
    ),
  ]),
  asstNode("n08", ts(), [
    text("`DefaultClock` 在 governor 0.8 中移到了 `clock` 之外，改用 `governor::DefaultKeyedRateLimiter`。"),
    call(
      "tu_edit2",
      "Edit",
      {
        file_path: "/Users/dev/Developer/lumen-api/src/middleware/rate_limit.rs",
        old_string: "use governor::{Quota, RateLimiter};",
        new_string: "use governor::{DefaultKeyedRateLimiter, Quota};",
      },
      { text: "The file /Users/dev/Developer/lumen-api/src/middleware/rate_limit.rs has been updated." },
    ),
    call(
      "tu_bash_persist",
      "Bash",
      { command: "cargo test --workspace", description: "运行全部测试" },
      {
        text: PERSISTED_TEXT,
        truncated: true,
        totalBytes: 184_320,
        persisted: { fileName: "bd2x9k1.txt", sizeBytes: 184_320 },
      },
    ),
  ]),
  asstNode("n09", ts(), [
    call(
      "tu_todo",
      "TodoWrite",
      {
        todos: [
          { content: "实现令牌桶中间件", status: "completed", activeForm: "实现令牌桶中间件" },
          { content: "接入 config.toml", status: "in_progress", activeForm: "接入 config.toml" },
          { content: "补充集成测试", status: "pending", activeForm: "补充集成测试" },
        ],
      },
      { text: "Todos have been modified successfully." },
    ),
    call(
      "tu_taskc",
      "TaskCreate",
      { subject: "为限流添加 Prometheus 指标", description: "暴露 rate_limited_total 计数器" },
      {
        text: "Task #1 created successfully",
        extraJson: JSON.stringify({ task: { id: "1", subject: "为限流添加 Prometheus 指标" } }),
      },
    ),
    call(
      "tu_taskupd",
      "TaskUpdate",
      { taskId: "1", status: "in_progress" },
      {
        text: "Updated task #1 status",
        extraJson: JSON.stringify({ task: { id: "1" }, statusChange: { from: "pending", to: "in_progress" } }),
      },
    ),
  ]),
  asstNode("n10", ts(), [
    text("超限时的响应有两种常见做法，需要你确认。"),
    // The user picked "Chat about this" after answering the first question.
    call(
      "tu_ask_clarify",
      "AskUserQuestion",
      { questions: clarifyQuestions },
      {
        text: [
          "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). To tell you how to proceed, the user said:",
          "The user wants to clarify these questions.",
          "    This means they may have additional information, context or questions for you.",
          "",
          "    Questions asked:",
          '- "按什么维度限流？"',
          "  Answer: API key",
          '- "默认额度是多少？"',
          "  (No answer provided)",
          "",
          "Note: The user's next message may contain a correction or preference.",
        ].join("\n"),
        isError: true,
        timestampMs: t + 60_000,
      },
    ),
    call(
      "tu_ask",
      "AskUserQuestion",
      { questions: askQuestions },
      {
        text: 'User has answered your questions: "限流超限时应返回什么？"="429 + Retry-After", "限流计数存在哪里？"="先放内存，留好换 Redis 的接口".',
        timestampMs: t + 90_000,
        extraJson: JSON.stringify({
          questions: askQuestions,
          answers: askAnswers,
          annotations: { "限流超限时应返回什么？": { notes: "Retry-After 用秒数" } },
        }),
      },
    ),
  ]),
  asstNode("n12", ts(), [
    text("并行派出三个子任务：梳理中间件、补测试、跑评审工作流。"),
    call(
      "tu_agent_explore",
      "Agent",
      {
        description: "梳理现有中间件注册方式",
        subagent_type: "Explore",
        prompt: "找出 lumen-api 里所有 tower Layer 的注册位置与顺序。",
      },
      { text: "共 3 处注册：trace、auth、cors；顺序见 src/middleware/mod.rs。" },
      { subagentId: "ag-sync1" },
    ),
    call(
      "tu_agent_tests",
      "Agent",
      {
        description: "为限流中间件补集成测试",
        subagent_type: "test-engineer",
        run_in_background: true,
        prompt: "为 RateLimitLayer 编写集成测试，覆盖超限、按 key 隔离与窗口恢复。",
      },
      { text: "Async agent launched successfully. agentId: ag-async1" },
      { subagentId: "ag-async1", notificationNodeId: "n-notify" },
    ),
    call(
      "tu_wf",
      "Workflow",
      { name: "pr-review", args: { base: "main" } },
      { text: "Workflow pr-review completed (3 phases, 2 agents)." },
      { workflowRunId: "wf-01" },
    ),
    call(
      "tu_mcp",
      "mcp__github__list_pull_requests",
      { owner: "lumen", repo: "lumen-api", state: "open" },
      {
        text: '[{"number":218,"title":"feat: 令牌桶限流","state":"open"},{"number":214,"title":"fix: 连接池泄漏","state":"open"}]',
      },
    ),
    call(
      "tu_bg_build",
      "Bash",
      { command: "cargo build --release > build.log 2>&1", description: "后台构建 release", run_in_background: true },
      {
        text: "Command running in background with ID: b-build1. Output is being written to: /tmp/tasks/b-build1.output.",
        extraJson: JSON.stringify({ stdout: "", stderr: "", interrupted: false, backgroundTaskId: "b-build1" }),
      },
      { notificationNodeId: "n-notify-bash" },
    ),
    call(
      "tu_artifact",
      "Artifact",
      { action: "read", url: "https://claude.ai/artifact/M0ckDesignCanvas" },
      { text: "[Artifact read]" },
    ),
    call(
      "tu_artifact_file",
      "Artifact",
      { action: "read", url: "https://claude.ai/artifact/M0ckDesignCanvas", path: "project/canvas.json" },
      { text: "{}" },
    ),
    call(
      "tu_artifact_publish",
      "Artifact",
      {
        url: "https://claude.ai/artifact/M0ckDesignCanvas",
        file_path:
          "/private/tmp/claude-501/-Users-dev-Developer-lumen-api/2f0c1e7a-5b1d-4c2e-9a77-0d3f6b8e41aa/scratchpad/artifact-files/9c1d2e3f-4a5b-6c7d-8e9f-0a1b2c3d4e5f/project/canvas.json",
      },
      { text: "Updated the Artifact" },
    ),
  ]),
  userNode(
    "n-notify",
    ts(),
    "<task-notification>\n<task-id>ag-async1</task-id>\n<status>completed</status>\n<summary>测试代理完成：新增 14 个用例，全部通过</summary>\n</task-notification>",
    {
      origin: {
        kind: "taskNotification",
        taskId: "ag-async1",
        toolUseId: "tu_agent_tests",
        status: "completed",
        summary: "测试代理完成：新增 14 个用例，全部通过",
      },
    },
  ),
  userNode(
    "n-notify-bash",
    ts(),
    '<task-notification>\n<task-id>b-build1</task-id>\n<status>completed</status>\n<summary>Background command "cargo build --release &gt; build.log 2&gt;&amp;1" completed (exit code 0)</summary>\n</task-notification>',
    {
      origin: {
        kind: "taskNotification",
        taskId: "b-build1",
        toolUseId: "tu_bg_build",
        status: "completed",
        summary: 'Background command "cargo build --release > build.log 2>&1" completed (exit code 0)',
      },
    },
  ),
  userNode(
    "n-teammate",
    t + 5_000,
    'Another Claude session sent a message:\n<teammate-message teammate_id="docs-writer" color="green" summary="限流文档已更新">\n已在 docs/rate-limit.md 补充配置说明：\n\n- `rate_limit.per_minute` 默认 120\n- 按 API key 隔离\n</teammate-message>\n\nThis came from another Claude session — not typed by your user, but very likely working on their behalf.',
    { origin: { kind: "teammate", teammateId: "docs-writer", color: "green", summary: "限流文档已更新" } },
  ),
  asstNode("n14", ts(), [
    text("对照一下监控面板的截图，确认限流指标已出现。"),
    call(
      "tu_img",
      "Read",
      { file_path: "/Users/dev/Pictures/grafana-ratelimit.png" },
      { text: "[image]", images: [screenshot] },
    ),
    // Resumes the finished background agent: its next notification names this call, not the Agent call.
    call(
      "tu_sendmsg",
      "SendMessage",
      { to: "ag-async1", message: "再补一个窗口恢复的边界用例。" },
      { text: "Resuming agent ag-async1" },
    ),
  ]),
  userNode(
    "n-notify2",
    ts(),
    '<task-notification>\n<task-id>ag-async1</task-id>\n<tool-use-id>tu_sendmsg</tool-use-id>\n<status>completed</status>\n<summary>Agent "为限流中间件补集成测试" finished</summary>\n</task-notification>',
    {
      origin: {
        kind: "taskNotification",
        taskId: "ag-async1",
        toolUseId: "tu_sendmsg",
        status: "completed",
        summary: 'Agent "为限流中间件补集成测试" finished',
      },
    },
  ),
  otherNode("n15", ts(), {
    kind: "system",
    subtype: "informational",
    level: "info",
    text: "已自动切换到 claude-sonnet-5-5（上一模型限额已用尽）",
  }),
  otherNode(
    "n-turn",
    ts(),
    { kind: "system", subtype: "turn_duration", level: null, text: "本轮耗时 4 分 12 秒" },
    { hidden: true },
  ),
  otherNode("n-cb", ts(), { kind: "compactBoundary", trigger: "auto", preTokens: 168_204 }),
  otherNode("n-cs", ts(), {
    kind: "compactSummary",
    text: "用户要求为 lumen-api 的 /v1/chat 增加按 API key 的令牌桶限流（默认 120/min）。已完成：新增 rate_limit.rs 并在中间件栈注册；修复 governor 0.8 的导入；全部 412 个测试通过。待办：把限流参数接入 config.toml，并补充 Prometheus 指标。",
  }),
  userNode("n16", ts(), "接下来把限流参数接到 config.toml，键名用 `[rate_limit] per_minute`。"),
  asstNode(
    "n17",
    ts(),
    [
      call(
        "tu_cfg_read",
        "Read",
        { file_path: "/Users/dev/Developer/lumen-api/src/config.rs" },
        { text: "     1\tpub struct Config {\n     2\t    pub listen: String,\n     3\t}" },
      ),
      call(
        "tu_cfg_edit",
        "Edit",
        {
          file_path: "/Users/dev/Developer/lumen-api/src/config.rs",
          old_string: "    pub listen: String,",
          new_string: "    pub listen: String,\n    pub rate_limit: RateLimitConfig,",
        },
        { text: "The file /Users/dev/Developer/lumen-api/src/config.rs has been updated." },
      ),
      text(
        "已接入配置，改动如下：\n\n| 文件 | 变更 |\n| --- | --- |\n| `src/config.rs` | 新增 `RateLimitConfig` |\n| `config.toml` | 新增 `[rate_limit]` 段 |\n\n```toml\n[rate_limit]\nper_minute = 120\n```\n\n默认值保持 120，未配置时沿用。",
      ),
    ],
    { usage: tok(900, 520, 24_000, 0) },
  ),
];

/** Abandoned alternative at anchor `n04`: replaces `n05..` when its head is chosen. */
export const richAlt: Node[] = [
  userNode("n05x", START + 5 * MIN, "顺便把 handler 里重复的鉴权逻辑也一起收敛掉。"),
  asstNode("n05y", START + 6 * MIN, [
    text("这会改动 handler 的签名，影响面较大。"),
    call(
      "tu_alt_edit",
      "Edit",
      {
        file_path: "/Users/dev/Developer/lumen-api/src/handlers/chat.rs",
        old_string: "let key = auth::extract(&req)?;",
        new_string: "let key = ctx.api_key();",
      },
      { text: "The file /Users/dev/Developer/lumen-api/src/handlers/chat.rs has been updated." },
    ),
  ]),
];

export const richBranch = {
  anchorKey: "n04",
  mainHeadId: "n05",
  altHeadId: "n05x",
  options: [
    {
      headId: "n05",
      preview: "先只加中间件，不要动 handler。",
      timestampMs: START + 4 * MIN,
      replyCount: 11,
      isMainLine: true,
    },
    {
      headId: "n05x",
      preview: "顺便把 handler 里重复的鉴权逻辑也一起收敛掉。",
      timestampMs: START + 5 * MIN,
      replyCount: 1,
      isMainLine: false,
    },
  ] satisfies BranchOption[],
};

/** Fragment / absorbed tool_result uuids that `resolve_jump` maps to display nodes. */
export const richAliases: Record<string, { nodeId: string; toolUseId: string | null }> = {
  "n04-frag2": { nodeId: "n04", toolUseId: null },
  "r-tu_bash_fail": { nodeId: "n07", toolUseId: "tu_bash_fail" },
};

export const richSubagents = [
  {
    agentId: "ag-sync1",
    agentType: "Explore",
    description: "梳理现有中间件注册方式",
    name: null,
    parentAgentId: null,
    spawnDepth: 0,
    toolUseId: "tu_agent_explore",
    workflowRunId: null,
    isAsync: false,
    status: "completed",
    model: "claude-haiku-4-5",
    messageCount: 8,
    toolCallCount: 5,
    tokens: tok(4200, 3100, 52_000, 1800),
    startedMs: START + 9 * MIN,
    endedMs: START + 10 * MIN,
    promptPreview: "找出 lumen-api 里所有 tower Layer 的注册位置与顺序。",
    finalText: "共 3 处注册：trace、auth、cors；顺序见 src/middleware/mod.rs。",
  },
  {
    agentId: "ag-nested1",
    agentType: "general-purpose",
    description: "核对 cors 层的配置来源",
    name: null,
    parentAgentId: "ag-sync1",
    spawnDepth: 1,
    toolUseId: "tu_nested_1",
    workflowRunId: null,
    isAsync: false,
    status: "completed",
    model: "claude-haiku-4-5",
    messageCount: 4,
    toolCallCount: 2,
    tokens: tok(1800, 900, 12_000, 0),
    startedMs: START + 9.4 * MIN,
    endedMs: START + 9.8 * MIN,
    promptPreview: "cors 层的允许来源是从哪里读取的？",
    finalText: "来自 config.toml 的 [cors] allowed_origins。",
  },
  {
    agentId: "ag-async1",
    agentType: "test-engineer",
    description: "为限流中间件补集成测试",
    name: null,
    parentAgentId: null,
    spawnDepth: 0,
    toolUseId: "tu_agent_tests",
    workflowRunId: null,
    isAsync: true,
    status: "completed",
    model: "claude-sonnet-5-5",
    messageCount: 21,
    toolCallCount: 14,
    tokens: tok(9800, 11_400, 210_000, 6200),
    startedMs: START + 11 * MIN,
    endedMs: START + 19 * MIN,
    promptPreview: "为 RateLimitLayer 编写集成测试，覆盖超限、按 key 隔离与窗口恢复。",
    finalText: "测试代理完成：新增 14 个用例，全部通过",
  },
  {
    agentId: "ag-orphan",
    agentType: "Explore",
    description: "检查 kestrel-cli 的重试策略",
    name: null,
    parentAgentId: null,
    spawnDepth: 0,
    toolUseId: null,
    workflowRunId: null,
    isAsync: false,
    status: null,
    model: "claude-haiku-4-5",
    messageCount: 3,
    toolCallCount: 1,
    tokens: tok(900, 400, 6000, 0),
    startedMs: START + 2 * MIN,
    endedMs: START + 3 * MIN,
    promptPreview: "kestrel-cli 的 HTTP 重试策略是什么？",
    finalText: null,
  },
  {
    agentId: "wf-ag-1",
    agentType: "code-reviewer",
    description: null,
    name: null,
    parentAgentId: null,
    spawnDepth: 0,
    toolUseId: null,
    workflowRunId: "wf-01",
    isAsync: false,
    status: "completed",
    model: "claude-sonnet-5-5",
    messageCount: 6,
    toolCallCount: 3,
    tokens: tok(3000, 2200, 40_000, 0),
    startedMs: START + 20 * MIN,
    endedMs: START + 22 * MIN,
    promptPreview: "评审限流中间件的并发安全",
    finalText: "未发现数据竞争。",
  },
  {
    agentId: "wf-ag-2",
    agentType: "security-reviewer",
    description: null,
    name: null,
    parentAgentId: null,
    spawnDepth: 0,
    toolUseId: null,
    workflowRunId: "wf-01",
    isAsync: false,
    status: "completed",
    model: "claude-sonnet-5-5",
    messageCount: 5,
    toolCallCount: 2,
    tokens: tok(2600, 1800, 33_000, 0),
    startedMs: START + 20 * MIN,
    endedMs: START + 23 * MIN,
    promptPreview: "评审限流绕过风险（X-Forwarded-For 伪造）",
    finalText: "建议以 API key 而非 IP 作为限流键，当前实现已满足。",
  },
] satisfies SubagentRun[];

export const richWorkflows = [
  {
    runId: "wf-01",
    name: "pr-review",
    summary: "对限流改动做并发与安全评审",
    status: "completed",
    toolUseId: "tu_wf",
    phases: [
      { index: 0, title: "收集改动", detail: "git diff main...HEAD" },
      { index: 1, title: "并行评审", detail: "并发安全 + 安全风险" },
      { index: 2, title: "汇总结论", detail: null },
    ],
    agents: [
      {
        agentId: "wf-ag-1",
        label: "并发评审",
        phaseIndex: 1,
        state: "completed",
        model: "claude-sonnet-5-5",
        tokens: 5200,
        toolCalls: 3,
        durationMs: 120_000,
        resultPreview: "未发现数据竞争。",
      },
      {
        agentId: "wf-ag-2",
        label: "安全评审",
        phaseIndex: 1,
        state: "completed",
        model: "claude-sonnet-5-5",
        tokens: 4400,
        toolCalls: 2,
        durationMs: 180_000,
        resultPreview: "建议以 API key 为限流键。",
      },
    ],
    durationMs: 210_000,
    totalTokens: 9600,
  },
] satisfies WorkflowRun[];

/** Nodes of each subagent's own transcript (scope = subagent). */
export const richAgentNodes: Record<string, Node[]> = {
  "ag-sync1": [
    userNode("ag1-u1", START + 9 * MIN, "找出 lumen-api 里所有 tower Layer 的注册位置与顺序。"),
    asstNode(
      "ag1-a1",
      START + 9.1 * MIN,
      [
        text("先搜索 Layer 的注册点。"),
        call(
          "tu_sub_grep",
          "Grep",
          { pattern: "RateLimitLayer|\\.layer\\(", path: "src" },
          {
            text: "src/middleware/mod.rs:5:    ServiceBuilder::new().layer(trace::layer()).layer(auth::layer())\nsrc/main.rs:31:    .layer(cors_layer(&cfg))",
          },
        ),
        call(
          "tu_nested_1",
          "Agent",
          {
            description: "核对 cors 层的配置来源",
            subagent_type: "general-purpose",
            prompt: "cors 层的允许来源是从哪里读取的？",
          },
          {
            text: "来自 config.toml 的 [cors] allowed_origins。",
          },
          { subagentId: "ag-nested1" },
        ),
      ],
      { model: "claude-haiku-4-5" },
    ),
    asstNode("ag1-a2", START + 9.9 * MIN, [text("共 3 处注册：trace、auth、cors；顺序见 src/middleware/mod.rs。")], {
      model: "claude-haiku-4-5",
    }),
  ],
  "ag-nested1": [
    userNode("ag1n-u1", START + 9.4 * MIN, "cors 层的允许来源是从哪里读取的？"),
    asstNode("ag1n-a1", START + 9.6 * MIN, [text("来自 config.toml 的 [cors] allowed_origins。")], {
      model: "claude-haiku-4-5",
    }),
  ],
  "ag-async1": [
    userNode("ag2-u1", START + 11 * MIN, "为 RateLimitLayer 编写集成测试，覆盖超限、按 key 隔离与窗口恢复。"),
    asstNode("ag2-a1", START + 12 * MIN, [
      text("在 tests/ 下新增 rate_limit.rs。"),
      call(
        "tu_sub_write",
        "Write",
        {
          file_path: "/Users/dev/Developer/lumen-api/tests/rate_limit.rs",
          content: "#[tokio::test]\nasync fn exceeds_quota_returns_429() {}",
        },
        {
          text: "File created successfully at: /Users/dev/Developer/lumen-api/tests/rate_limit.rs",
        },
      ),
    ]),
    asstNode("ag2-a2", START + 18 * MIN, [text("新增 14 个用例，全部通过。")]),
    userNode("ag2-u2", START + 19 * MIN, "再补一个窗口恢复的边界用例。", { origin: { kind: "coordinator" } }),
    asstNode("ag2-a3", START + 20 * MIN, [text("补上了窗口恢复的边界用例，共 15 个，全部通过。")]),
  ],
  "ag-orphan": [
    userNode("ago-u1", START + 2 * MIN, "kestrel-cli 的 HTTP 重试策略是什么？"),
    asstNode("ago-a1", START + 2.5 * MIN, [text("指数退避，最多 5 次，基数 200ms。")], { model: "claude-haiku-4-5" }),
  ],
  "wf-ag-1": [
    userNode("wf1-u1", START + 20 * MIN, "评审限流中间件的并发安全"),
    asstNode("wf1-a1", START + 21 * MIN, [text("未发现数据竞争。")]),
  ],
  "wf-ag-2": [
    userNode("wf2-u1", START + 20 * MIN, "评审限流绕过风险（X-Forwarded-For 伪造）"),
    asstNode("wf2-a1", START + 22 * MIN, [text("建议以 API key 而非 IP 作为限流键，当前实现已满足。")]),
  ],
};
