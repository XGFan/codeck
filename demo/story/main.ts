// 主 Session：为 lumen-api 的 /v1/chat 加令牌桶限流。覆盖对话记录里的大部分消息类型。

import { launch, notify } from "../../scripts/demo/agents.ts";
import {
  apiErrorMessage,
  apiRetry,
  askAnswered,
  bang,
  bash,
  bashBackground,
  command,
  compact,
  edit,
  editFailed,
  grep,
  imagePrompt,
  informational,
  localCommand,
  mcp,
  prompt,
  read,
  shellNotification,
  taskCreate,
  taskNotification,
  taskUpdate,
  webFetch,
  webSearch,
  write,
} from "../../scripts/demo/kit.ts";
import { Session, type Writer, png, shape } from "../../scripts/demo/lib.ts";

export const MAIN_ID = "5d3e8a41-7c2f-4b9a-9e61-2f8c0d4a7b15";
const CWD = "/Users/dev/code/lumen-api";
const f = (p: string) => `${CWD}/${p}`;

const MAIN_RS = `use axum::{routing::post, Router};
use crate::{config::Config, handlers::chat, middleware::auth};

pub fn build_router(cfg: &Config) -> Router {
    Router::new()
        .route("/v1/chat", post(chat))
        .layer(auth::layer(cfg))
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http())
}
`;

const CONFIG_RS = `#[derive(Deserialize, Default)]
pub struct Config {
    pub listen: SocketAddr,
    pub redis_url: Option<String>,
}
`;

const RATELIMIT_RS = `use std::{collections::HashMap, time::{Duration, Instant}};
use parking_lot::Mutex;

/// One token bucket per API key: \`burst\` tokens, refilled at \`per_minute\` per minute.
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    per_minute: u32,
    burst: u32,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(per_minute: u32, burst: u32) -> Self {
        Self { buckets: Mutex::default(), per_minute, burst }
    }

    /// \`Ok\` when the request may go through, else how long to wait.
    pub fn check(&self, key: &str) -> Result<(), Duration> {
        let mut map = self.buckets.lock();
        let b = map.entry(key.into()).or_insert(Bucket { tokens: self.burst as f64, last: Instant::now() });
        let rate = self.per_minute as f64 / 60.0;
        b.tokens = (b.tokens + b.last.elapsed().as_secs_f64() * rate).min(self.burst as f64);
        b.last = Instant::now();
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - b.tokens) / rate))
        }
    }
}
`;

const TEST_RS = `use lumen_api::middleware::ratelimit::RateLimiter;

#[test]
fn burst_allows_twenty() {
    let l = RateLimiter::new(120, 20);
    assert!((0..20).all(|_| l.check("k").is_ok()));
    assert!(l.check("k").is_err());
}

#[test]
fn separate_keys_do_not_share() {
    let l = RateLimiter::new(120, 1);
    assert!(l.check("a").is_ok());
    assert!(l.check("b").is_ok());
}
`;

const PAGE_429 = png(
  320,
  200,
  [255, 255, 255],
  [
    [0, 0, 320, 24, [233, 233, 230]],
    [100, 64, 120, 18, [209, 75, 60]],
    [60, 98, 200, 8, [201, 201, 197]],
    [90, 114, 140, 8, [201, 201, 197]],
    [130, 140, 60, 20, [10, 95, 204]],
  ],
);

export function mainSession(): Session {
  const s = new Session({
    id: MAIN_ID,
    project: "lumen-api",
    branch: "feat/rate-limit",
    start: "2026-11-04T06:46:00.000Z",
    title: "为 /v1/chat 接口增加令牌桶限流中间件",
  });
  const w = s.main;

  // Session start hook (hidden unless 「显示系统消息」).
  const hook = shape("attachment.hook_success");
  Object.assign(hook.attachment, {
    hookName: "SessionStart:startup",
    hookEvent: "SessionStart",
    command: "~/.claude/hooks/session-start.sh",
    content: "",
    stdout: '{"continue":true}\n',
    stderr: "",
    exitCode: 0,
    durationMs: 312,
    toolUseID: s.ids.uuid(),
  });
  w.push(hook);

  turn1(w);
  turn2(w);
  turn3(w);
  turn4(w);
  turn5(w);
  turn6(w);
  turn7(w);
  turn8(w);
  turn9(w);
  return s;
}

/** 1 · 普通提示词：读、建任务、写、改、搜、测，回答带表格与代码块。 */
function turn1(w: Writer) {
  prompt(w, "给 /v1/chat 加令牌桶限流，超限返回 429，计数先放内存。", 120_000);
  const t0 = w.now;
  w.think(
    "先看路由在哪里注册、配置怎么读。限流应该放在 auth 之后：匿名请求在 auth 就被拒了，不该占令牌。计数先用进程内的 HashMap，后面再看要不要换 Redis。",
    4200,
  );
  w.say("先看现有的路由注册和配置读取。");
  w.calls([
    { name: "Read", input: { file_path: f("src/main.rs") }, out: read(f("src/main.rs"), MAIN_RS) },
    { name: "Read", input: { file_path: f("src/config.rs") }, out: read(f("src/config.rs"), CONFIG_RS, 120) },
  ]);
  w.calls([
    {
      name: "TaskCreate",
      input: { subject: "写令牌桶 RateLimiter", description: "每个 API key 一个桶" },
      out: taskCreate("1", "写令牌桶 RateLimiter"),
    },
    {
      name: "TaskCreate",
      input: { subject: "接到 /v1/chat 路由", description: "放在 auth 之后" },
      out: taskCreate("2", "接到 /v1/chat 路由"),
    },
    {
      name: "TaskCreate",
      input: { subject: "补单元测试", description: "突发、按 key 隔离" },
      out: taskCreate("3", "补单元测试"),
    },
  ]);
  w.call("TaskUpdate", { taskId: "1", status: "in_progress" }, taskUpdate("1", "pending", "in_progress"));
  w.call(
    "Write",
    { file_path: f("src/middleware/ratelimit.rs"), content: RATELIMIT_RS },
    write(f("src/middleware/ratelimit.rs"), RATELIMIT_RS),
  );
  w.call("TaskUpdate", { taskId: "1", status: "completed" }, taskUpdate("1", "in_progress", "completed"));
  w.say("RateLimiter 写好了，接到路由上，放在 auth 之后。");
  w.call(
    "Edit",
    {
      file_path: f("src/main.rs"),
      old_string: "        .layer(auth::layer(cfg))",
      new_string: "        .layer(ratelimit::layer(cfg))\n        .layer(auth::layer(cfg))",
    },
    edit(
      f("src/main.rs"),
      "        .layer(auth::layer(cfg))",
      "        .layer(ratelimit::layer(cfg))\n        .layer(auth::layer(cfg))",
      [
        {
          oldStart: 5,
          oldLines: 4,
          newStart: 5,
          newLines: 5,
          lines: [
            "     Router::new()",
            '         .route("/v1/chat", post(chat))',
            "+        .layer(ratelimit::layer(cfg))",
            "         .layer(auth::layer(cfg))",
            "         .layer(tower_http::cors::CorsLayer::permissive())",
          ],
        },
      ],
    ),
  );
  w.call(
    "Grep",
    { pattern: "RateLimiter", path: "src", glob: "*.rs", output_mode: "content", "-n": true },
    grep(
      [
        "src/middleware/ratelimit.rs:5:pub struct RateLimiter {",
        "src/middleware/ratelimit.rs:16:impl RateLimiter {",
        "src/middleware/mod.rs:3:pub use ratelimit::RateLimiter;",
      ],
      ["src/middleware/ratelimit.rs", "src/middleware/mod.rs"],
    ),
  );
  w.call("Write", { file_path: f("tests/ratelimit.rs"), content: TEST_RS }, write(f("tests/ratelimit.rs"), TEST_RS));
  w.call(
    "Bash",
    { command: "cargo test --test ratelimit", description: "运行限流单元测试" },
    bash(
      "   Compiling lumen-api v0.4.2 (/Users/dev/code/lumen-api)\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.41s\n     Running tests/ratelimit.rs\n\nrunning 2 tests\ntest burst_allows_twenty ... ok\ntest separate_keys_do_not_share ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; finished in 0.01s",
      { ms: 8200 },
    ),
  );
  w.calls([
    { name: "TaskUpdate", input: { taskId: "2", status: "completed" }, out: taskUpdate("2", "pending", "completed") },
    { name: "TaskUpdate", input: { taskId: "3", status: "completed" }, out: taskUpdate("3", "pending", "completed") },
  ]);
  w.say(
    "限流中间件已接入 `/v1/chat`：每个 API key 一个令牌桶，默认每分钟 120 次，可突发 20 次，超限返回 `429` 并带 `Retry-After`。\n\n## 改动\n\n| 文件 | 变更 |\n|---|---|\n| `src/middleware/ratelimit.rs` | 新增 `RateLimiter` |\n| `src/main.rs` | 在 auth 之后挂上限流层 |\n| `tests/ratelimit.rs` | 2 个单元测试 |\n\n```rust\nlet limiter = RateLimiter::new(120, 20);\nlimiter.check(&api_key)?;\n```\n\n- 计数在进程内存里，多实例部署时各算各的。\n- 测试都通过了。",
    { end: true },
  );
  w.endTurn(t0, 24);
  w.s.marks.set("afterTurn1", w.last!);
}

/** 2 · 带代码的提示词：编辑失败后重试、clippy 失败后修好。 */
function turn2(w: Writer) {
  prompt(
    w,
    "接下来把限流参数接到 config.toml，键名用 `[rate_limit] per_minute`，例如：\n```toml\n[rate_limit]\nper_minute = 120\n```\n没配置时沿用 120。",
    180_000,
  );
  const t0 = w.now;
  w.call(
    "Edit",
    {
      file_path: f("src/lib.rs"),
      old_string: "use crate::limits::RateLimiter;",
      new_string: "use crate::middleware::RateLimiter;",
      replace_all: true,
    },
    editFailed("String to replace not found in file.\nString: use crate::limits::RateLimiter;"),
  );
  w.say("lib.rs 里没有这个 import，直接改 config.rs。");
  w.call(
    "Edit",
    {
      file_path: f("src/config.rs"),
      old_string: "    pub redis_url: Option<String>,\n}",
      new_string: "    pub redis_url: Option<String>,\n    #[serde(default)]\n    pub rate_limit: RateLimitConfig,\n}",
    },
    edit(
      f("src/config.rs"),
      "    pub redis_url: Option<String>,\n}",
      "    pub redis_url: Option<String>,\n    #[serde(default)]\n    pub rate_limit: RateLimitConfig,\n}",
      [
        {
          oldStart: 122,
          oldLines: 3,
          newStart: 122,
          newLines: 5,
          lines: [
            "     pub listen: SocketAddr,",
            "     pub redis_url: Option<String>,",
            "+    #[serde(default)]",
            "+    pub rate_limit: RateLimitConfig,",
            " }",
          ],
        },
      ],
    ),
  );
  w.call(
    "Edit",
    {
      file_path: f("src/middleware/ratelimit.rs"),
      old_string: "per_minute: u32",
      new_string: "per_minute: NonZeroU32",
      replace_all: true,
    },
    edit(
      f("src/middleware/ratelimit.rs"),
      "per_minute: u32",
      "per_minute: NonZeroU32",
      [
        {
          oldStart: 7,
          oldLines: 1,
          newStart: 7,
          newLines: 1,
          lines: ["-    per_minute: u32,", "+    per_minute: NonZeroU32,"],
        },
        {
          oldStart: 17,
          oldLines: 1,
          newStart: 17,
          newLines: 1,
          lines: [
            "-    pub fn new(per_minute: u32, burst: u32) -> Self {",
            "+    pub fn new(per_minute: NonZeroU32, burst: u32) -> Self {",
          ],
        },
      ],
      { replaceAll: true },
    ),
  );
  w.call(
    "Bash",
    { command: "cargo clippy --all-targets -- -D warnings", description: "运行 clippy" },
    bash("    Checking lumen-api v0.4.2 (/Users/dev/code/lumen-api)", {
      exit: 101,
      stderr:
        "error: this `if` has identical blocks\n  --> src/middleware/ratelimit.rs:31:9\n   |\n31 |         if b.tokens >= 1.0 {\n   |         ^^^^^^^^^^^^^^^^^^^\nerror: could not compile `lumen-api` (lib) due to 1 previous error",
      ms: 14_200,
    }),
  );
  w.think("clippy 报的是 check 里两个分支都在改 b.last，把更新挪到 if 之前就行。", 2600);
  w.call(
    "Edit",
    {
      file_path: f("src/middleware/ratelimit.rs"),
      old_string: "        b.last = Instant::now();\n        if b.tokens >= 1.0 {",
      new_string: "        b.last = Instant::now();\n\n        if b.tokens >= 1.0 {",
    },
    edit(
      f("src/middleware/ratelimit.rs"),
      "        b.last = Instant::now();\n        if b.tokens >= 1.0 {",
      "        b.last = Instant::now();\n\n        if b.tokens >= 1.0 {",
      [
        {
          oldStart: 29,
          oldLines: 2,
          newStart: 29,
          newLines: 3,
          lines: ["         b.last = Instant::now();", "+", "         if b.tokens >= 1.0 {"],
        },
      ],
    ),
  );
  w.call(
    "Bash",
    { command: "cargo clippy --all-targets -- -D warnings", description: "再跑一次 clippy" },
    bash(
      "    Checking lumen-api v0.4.2 (/Users/dev/code/lumen-api)\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.02s",
      { ms: 9800 },
    ),
  );
  w.say(
    "已接入配置，`[rate_limit]` 段没写时沿用默认值 120。\n\n```toml\n[rate_limit]\nper_minute = 120\nburst = 20\n```\n\nclippy 也干净了。",
    { end: true },
  );
  w.endTurn(t0, 18);
}

/** 3 · 斜杠命令：并行派出两个后台 Subagent，结束通知回来后汇总。 */
function turn3(w: Writer) {
  command(
    w,
    "review",
    "重点看限流中间件的并发",
    "Review the current changes. Focus: 重点看限流中间件的并发\n\nRead the diff, list risks by severity, and verify each finding before reporting it.",
    240_000,
  );
  const t0 = w.now;
  w.say("我派两个 agent 并行查：一个看 Redis 连接池，一个看现有的 429 处理。");
  const [a, b] = launch(w, [
    {
      type: "Explore",
      description: "查 Redis 连接池的配置",
      model: "haiku",
      prompt:
        "在 /Users/dev/code/lumen-api 里找 Redis 连接池的配置：最大连接数、超时、在哪里建的。只读，列出文件与行号。",
      run: (sub) => {
        sub.call(
          "Bash",
          { command: 'rg -n "Pool|redis" src --type rust', description: "搜索连接池" },
          bash(
            "src/store.rs:12:    let pool = Pool::builder().max_size(16).connection_timeout(Duration::from_secs(2))\nsrc/store.rs:18:pub async fn redis(cfg: &Config) -> Result<Pool<RedisConnectionManager>> {",
            { ms: 600 },
          ),
        );
        sub.call(
          "Read",
          { file_path: f("src/store.rs"), offset: 10, limit: 12 },
          read(
            f("src/store.rs"),
            'pub async fn redis(cfg: &Config) -> Result<Pool<RedisConnectionManager>> {\n    let manager = RedisConnectionManager::new(cfg.redis_url.as_deref().unwrap_or("redis://127.0.0.1"))?;\n    let pool = Pool::builder().max_size(16).connection_timeout(Duration::from_secs(2))\n        .build(manager).await?;\n    Ok(pool)\n}',
            10,
          ),
        );
        return "连接池在 src/store.rs 的 `redis()` 里建：最大 16 个连接，取连接超时 2 秒，地址取 `redis_url`，没配时用本机。";
      },
    },
    {
      type: "Explore",
      description: "查现有的 429 处理",
      model: "haiku",
      prompt: "在 /Users/dev/code/lumen-api 里查有没有地方已经返回 429 或处理限流，列出位置。",
      run: (sub) => {
        sub.call(
          "Bash",
          { command: 'rg -n "429|TOO_MANY_REQUESTS" src', description: "搜索 429" },
          bash("", { exit: 1, ms: 400 }),
        );
        return "还没有任何地方返回 429；错误类型都在 src/error.rs 的 `ApiError` 里，可以加一个 `TooManyRequests(Duration)` 变体。";
      },
    },
  ]);
  w.say("两个 agent 在后台查，结束了会通知。", { end: true });
  w.endTurn(t0, 6, 2);
  notify(w, a!, 15_000);
  const t1 = w.now;
  w.say("Redis 那边记下了，等另一个结果。", { end: true });
  w.endTurn(t1, 2, 1);
  notify(w, b!, 5_000);
  const t2 = w.now;
  w.say(
    "两边都查完了：\n\n- Redis 连接池最大 16 个连接，取连接超时 2 秒，换成 Redis 计数时要注意连接数。\n- 还没有地方返回 429，我在 `ApiError` 里加了 `TooManyRequests`，中间件超限时用它。\n\n并发上，`RateLimiter` 用一把 `Mutex` 护着整张表，持锁只做一次取令牌，压力不大时够用。",
    { end: true },
  );
  w.endTurn(t2, 2);
}

/** 4 · 后台：Subagent 补集成测试 + 后台构建（失败），两个结束通知。 */
function turn4(w: Writer) {
  prompt(w, "后台补集成测试，同时打一个 release 包。", 300_000);
  const t0 = w.now;
  const [agent] = launch(w, [
    {
      type: "general-purpose",
      description: "为限流中间件补集成测试",
      prompt:
        "为 src/middleware/ratelimit.rs 补集成测试，放在 tests/ratelimit_http.rs：突发 20 次都放行；第 21 次返回 429 且带 Retry-After；不同 API key 互不影响。用 tower::ServiceExt::oneshot 直接调 Router，不起真实端口。跑完把通过数报回来。",
      run: (sub) => {
        sub.say("先看路由怎么建，再写测试。");
        sub.call("Read", { file_path: f("src/main.rs") }, read(f("src/main.rs"), MAIN_RS));
        sub.call(
          "Write",
          {
            file_path: f("tests/ratelimit_http.rs"),
            content: "// 14 个用例：突发、回填、按 key 隔离、429 与 Retry-After\n",
          },
          write(f("tests/ratelimit_http.rs"), "// 14 个用例：突发、回填、按 key 隔离、429 与 Retry-After\n"),
        );
        sub.call(
          "Bash",
          { command: "cargo test --test ratelimit_http", description: "运行集成测试" },
          bash("running 14 tests\n..............\ntest result: ok. 14 passed; 0 failed; 0 ignored; finished in 1.12s", {
            ms: 21_000,
          }),
        );
        return "新增 tests/ratelimit_http.rs，14 个用例全部通过（1.12 秒）：突发、每秒回填、按 key 隔离、429 与 Retry-After。";
      },
    },
  ]);
  const id = w.s.ids.shell();
  const build = w.call(
    "Bash",
    { command: "cargo build --release > build.log 2>&1", description: "后台构建 release", run_in_background: true },
    bashBackground(id, CWD),
  );
  w.say("集成测试交给后台 agent，release 构建也在后台跑，结束了会通知。", { end: true });
  w.endTurn(t0, 5, 1);

  taskNotification(w, shellNotification(w.s, id, build._toolUseId, "后台构建 release", 101), 90_000);
  const t1 = w.now;
  w.call(
    "Bash",
    { command: "tail -5 build.log", description: "看构建日志" },
    bash(
      "   Compiling lumen-api v0.4.2 (/Users/dev/code/lumen-api)\nerror[E0432]: unresolved import `crate::limits`\n --> src/bin/bench.rs:3:5\n  |\n3 | use crate::limits::RateLimiter;",
      { ms: 300 },
    ),
  );
  w.call(
    "Edit",
    {
      file_path: f("src/bin/bench.rs"),
      old_string: "use crate::limits::RateLimiter;",
      new_string: "use lumen_api::middleware::RateLimiter;",
    },
    edit(f("src/bin/bench.rs"), "use crate::limits::RateLimiter;", "use lumen_api::middleware::RateLimiter;", [
      {
        oldStart: 3,
        oldLines: 1,
        newStart: 3,
        newLines: 1,
        lines: ["-use crate::limits::RateLimiter;", "+use lumen_api::middleware::RateLimiter;"],
      },
    ]),
  );
  w.say("release 构建失败是因为 `src/bin/bench.rs` 还引用旧路径 `crate::limits`，已改成 `lumen_api::middleware`。", {
    end: true,
  });
  w.endTurn(t1, 4, 1);

  notify(w, agent!, 30_000);
  const t2 = w.now;
  w.say("集成测试补好了：`tests/ratelimit_http.rs` 14 个用例全部通过。", { end: true });
  w.endTurn(t2, 2);
}

/** 5 · 向你提问（已回答）。 */
function turn5(w: Writer) {
  prompt(w, "要支持多实例部署，计数放哪合适？你定之前先问我。", 240_000);
  const t0 = w.now;
  const q = {
    question: "限流计数放在哪里？",
    header: "存储",
    multiSelect: false,
    options: [
      { label: "Redis", description: "进程外共享，多实例一致" },
      { label: "内存 LRU", description: "最快，重启即失" },
      { label: "不加缓存", description: "先观察命中率" },
    ],
  };
  w.call("AskUserQuestion", { questions: [q] }, askAnswered([q], { [q.question]: "Redis" }));
  w.say(
    "好，用 Redis：每个 key 一个 `INCR` + `EXPIRE` 的固定窗口近似令牌桶，连接复用现有的连接池。下一步我先写接口，内存实现保留给测试用。",
    { end: true },
  );
  w.endTurn(t0, 4);
}

/** 6 · 带图片的提示词。 */
function turn6(w: Writer) {
  imagePrompt(w, "这个 429 页面的样式不对，见截图", PAGE_429, 300_000);
  const t0 = w.now;
  w.call(
    "Read",
    { file_path: f("templates/429.html") },
    read(
      f("templates/429.html"),
      '<main class="error">\n  <h1>请求过于频繁</h1>\n  <p>请在 {{retry_after}} 秒后重试。</p>\n  <a class="btn" href="/">返回首页</a>\n</main>',
    ),
  );
  w.call(
    "Edit",
    {
      file_path: f("templates/429.html"),
      old_string: '<main class="error">',
      new_string: '<main class="error error--center">',
    },
    edit(f("templates/429.html"), '<main class="error">', '<main class="error error--center">', [
      {
        oldStart: 1,
        oldLines: 1,
        newStart: 1,
        newLines: 1,
        lines: ['-<main class="error">', '+<main class="error error--center">'],
      },
    ]),
  );
  w.say("截图里标题和按钮没居中，是 429 页漏了 `error--center`，已补上，和 404 页一致。", { end: true });
  w.endTurn(t0, 4);
}

/** 7 · 查资料：WebFetch + WebSearch，回答带链接。 */
function turn7(w: Writer) {
  prompt(w, "Retry-After 用秒还是 HTTP 日期？查一下规范。", 240_000);
  const t0 = w.now;
  w.calls([
    {
      name: "WebFetch",
      input: { url: "https://www.rfc-editor.org/rfc/rfc6585#section-4", prompt: "429 状态码对 Retry-After 的要求" },
      out: webFetch(
        "https://www.rfc-editor.org/rfc/rfc6585#section-4",
        "RFC 6585 §4: The 429 status code indicates that the user has sent too many requests in a given amount of time. The response representations SHOULD include details explaining the condition, and MAY include a Retry-After header indicating how long to wait before making a new request.",
      ),
    },
    {
      name: "WebSearch",
      input: { query: "Retry-After header delay-seconds vs HTTP-date" },
      out: webSearch(
        "Retry-After header delay-seconds vs HTTP-date",
        [{ title: "RFC 9110 §10.2.3 Retry-After", url: "https://www.rfc-editor.org/rfc/rfc9110#section-10.2.3" }],
        "Retry-After may be an HTTP-date or a number of seconds (delay-seconds).",
      ),
    },
  ]);
  w.say(
    "用秒就行：[RFC 9110 §10.2.3](https://www.rfc-editor.org/rfc/rfc9110#section-10.2.3) 允许 `delay-seconds` 或 HTTP 日期，[RFC 6585 §4](https://www.rfc-editor.org/rfc/rfc6585#section-4) 只说 429 *可以* 带 `Retry-After`。秒数不依赖时钟同步，现在的实现就是这样。",
    { end: true },
  );
  w.endTurn(t0, 4);
}

/** 8 · compact 之后：开 PR（MCP）、! 命令、重命名（本地命令）。 */
function turn8(w: Writer) {
  compact(
    w,
    "## Primary Request and Intent\n为 lumen-api 的 /v1/chat 加令牌桶限流：超限返回 429 与 Retry-After，参数来自 config.toml 的 [rate_limit]，多实例时计数放 Redis。\n\n## Pending Tasks\n- 开 PR\n- Redis 计数实现",
    168_240,
  );
  informational(
    w,
    'Compacted while idle, before the prompt cache expired · turn off with "idleCompaction": false in settings.json',
  );
  prompt(w, "开个 PR。", 600_000);
  const t0 = w.now;
  w.call(
    "mcp__github__create_pull_request",
    {
      owner: "lumen",
      repo: "lumen-api",
      title: "feat: 令牌桶限流",
      head: "feat/rate-limit",
      base: "main",
      body: "为 /v1/chat 加令牌桶限流，超限返回 429 与 Retry-After。",
    },
    mcp("Created pull request #42: https://github.com/lumen/lumen-api/pull/42"),
  );
  w.say("PR 开好了：[lumen/lumen-api#42](https://github.com/lumen/lumen-api/pull/42)。", { end: true });
  w.endTurn(t0, 2);
  bang(w, "git status --short", " M src/config.rs\n M src/main.rs\n?? tests/ratelimit_http.rs", 120_000);
  localCommand(w, "rename", "rate-limit", "Session renamed to: rate-limit");
}

/** 9 · 回退重发分出 Branch；选中的那支先遇到 API 错误（重试两次后放弃），再继续。 */
function turn9(w: Writer) {
  const before = w.last;
  prompt(w, "换个思路：用滑动窗口，不用令牌桶。", 300_000);
  const t0 = w.now;
  w.say("滑动窗口要为每个 key 存最近一分钟的请求时间戳，内存随 QPS 涨；令牌桶只存两个数。要换的话我先写个对比压测。", {
    end: true,
  });
  w.endTurn(t0, 2);
  // Rewound to before that prompt and asked something else: a Branch with two options; this one is the Main Line.
  w.last = before;
  prompt(w, "保持令牌桶，把 burst 调成 20，滑动窗口以后再说。", 120_000);
  const t1 = w.now;
  apiRetry(w, 1);
  apiRetry(w, 2);
  apiErrorMessage(w);
  w.endTurn(t1, 1);
  prompt(w, "继续", 90_000);
  const t2 = w.now;
  w.call(
    "Edit",
    { file_path: f("config.toml"), old_string: "burst = 10", new_string: "burst = 20" },
    edit(f("config.toml"), "burst = 10", "burst = 20", [
      { oldStart: 3, oldLines: 1, newStart: 3, newLines: 1, lines: ["-burst = 10", "+burst = 20"] },
    ]),
  );
  w.say("改好了：默认 `burst = 20`。滑动窗口先不动。", { end: true });
  w.endTurn(t2, 3);
}
