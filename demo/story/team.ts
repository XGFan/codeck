// Teammate Session：派两个 teammate 并行（Redis 计数、压测），它们发回主线的各种消息；另一个会话发来的消息。

import { spawnTeammates } from "../../scripts/demo/agents.ts";
import { bash, edit, peerMessage, prompt, teammateMessage, write } from "../../scripts/demo/kit.ts";
import { Session } from "../../scripts/demo/lib.ts";

const CWD = "/Users/dev/code/lumen-api";
const f = (p: string) => `${CWD}/${p}`;

export function teamSession(): Session {
  const s = new Session({
    id: "9b14c6e2-3f7a-4d58-a0c1-6e2b9f8d3a47",
    project: "lumen-api",
    branch: "feat/redis-ratelimit",
    start: "2026-11-05T01:40:00.000Z",
    title: "Redis 计数与压测并行",
    slug: "parallel-redis-bench",
  });
  const w = s.main;
  const team = `session-${s.o.id.slice(0, 8)}`;

  prompt(w, "两条线并行：A 把限流计数换成 Redis，B 写压测脚本对比内存与 Redis 两种实现。", 60_000);
  const t0 = w.now;
  w.say("派两个 teammate：redis-store 做 Redis 计数，bench-runner 写压测。");
  spawnTeammates(w, team, [
    {
      name: "redis-store",
      color: "blue",
      type: "general-purpose",
      description: "Redis 计数实现",
      prompt:
        "把 src/middleware/ratelimit.rs 的计数换成 Redis：每个 key 一个 INCR + EXPIRE 窗口，连接用 src/store.rs 的连接池；内存实现保留给测试。做完把提交号报回来。",
      run: (sub) => {
        sub.call(
          "Write",
          {
            file_path: f("src/middleware/ratelimit/redis.rs"),
            content: "pub struct RedisCounter { pool: Pool<RedisConnectionManager> }\n",
          },
          write(
            f("src/middleware/ratelimit/redis.rs"),
            "pub struct RedisCounter { pool: Pool<RedisConnectionManager> }\n",
          ),
        );
        sub.call(
          "Bash",
          { command: "cargo test ratelimit", description: "运行限流测试" },
          bash("test result: ok. 16 passed; 0 failed; 0 ignored", { ms: 12_000 }),
        );
        return "Redis 计数完成，提交 7c41e2a。";
      },
    },
    {
      name: "bench-runner",
      color: "green",
      type: "general-purpose",
      description: "压测脚本",
      prompt:
        "写 scripts/bench.sh：用 wrk 在 1k QPS 下压 /v1/chat 30 秒，分别跑内存与 Redis 两种实现，输出 p50 / p99 延迟与 429 比例。",
      run: (sub) => {
        sub.call(
          "Write",
          {
            file_path: f("scripts/bench.sh"),
            content: "#!/bin/sh\nwrk -t4 -c64 -d30s -R1000 http://localhost:8080/v1/chat\n",
          },
          write(f("scripts/bench.sh"), "#!/bin/sh\nwrk -t4 -c64 -d30s -R1000 http://localhost:8080/v1/chat\n"),
        );
        sub.call(
          "Bash",
          { command: "sh scripts/bench.sh", description: "运行压测" },
          bash("", { exit: 127, stderr: "scripts/bench.sh: line 2: wrk: command not found", ms: 300 }),
        );
        return "压测脚本写好了，但本机没有 wrk，跑不起来。";
      },
    },
  ]);
  w.say("两个 teammate 都派出去了，有结果会发回来。", { end: true });
  w.endTurn(t0, 4, 2);

  // A report, then the end of redis-store's round with its result (one relayed message each).
  teammateMessage(
    w,
    [
      {
        id: "redis-store",
        color: "blue",
        summary: "Redis 计数接口写好了，在跑测试",
        body: "Redis 计数接口写好了：`RedisCounter` 实现了 `Counter` trait，内存版改名 `MemoryCounter` 留给测试。正在跑全部限流测试。",
      },
    ],
    600_000,
  );
  teammateMessage(
    w,
    [
      {
        id: "redis-store",
        color: "blue",
        body: JSON.stringify({
          type: "idle_notification",
          from: "redis-store",
          timestamp: new Date(w.now + 240_000).toISOString(),
          idleReason: "available",
          result:
            'Redis 计数完成，在 feat/redis-ratelimit 上一个提交 `7c41e2a`，没有推送。\n\n**改动**\n- `Counter` trait：`MemoryCounter`（原实现）与 `RedisCounter`（新）。\n- `RedisCounter` 每个 key 用 `INCR` + `EXPIRE 60`，连接取自 `store::redis()` 的连接池。\n- 配置 `[rate_limit] backend = "redis" | "memory"`，默认 memory。\n\n**验证**\n- `cargo test ratelimit`：16 个全部通过（含 2 个 Redis 用例，需本机 Redis）。\n- 手动起两个实例共用一个 Redis：第 121 次请求在两边都返回 429。\n\n**没做**\n- Redis 断开时的降级（现在直接 500），等你定是放行还是拒绝。',
        }),
      },
    ],
    240_000,
  );
  const t1 = w.now;
  w.say("redis-store 完成了。Redis 断开时我倾向放行（fail open），免得 Redis 抖动把接口打挂。", { end: true });
  w.endTurn(t1, 2, 1);

  // bench-runner: its round fails (API error), then it goes idle; the lead assigns it a task.
  teammateMessage(
    w,
    [
      {
        id: "bench-runner",
        color: "green",
        body: JSON.stringify({
          type: "idle_notification",
          from: "bench-runner",
          timestamp: new Date(w.now + 300_000).toISOString(),
          idleReason: "failed",
          failureReason: "API Error: Connection dropped (ECONNRESET)",
        }),
      },
    ],
    300_000,
  );
  const t2 = w.now;
  w.call(
    "SendMessage",
    {
      to: "bench-runner",
      summary: "改用 oha 压测",
      message: "本机没有 wrk，改用 oha（`brew install oha`），其余不变。",
    },
    {
      content: "Message sent to bench-runner's inbox",
      toolUseResult: {
        success: true,
        message: "Message sent to bench-runner's inbox",
        pin: null,
        resumedAgentId: null,
        revivedAgentId: null,
      },
      ms: 200,
    },
  );
  w.say("让 bench-runner 改用 oha 再跑。", { end: true });
  w.endTurn(t2, 3, 1);
  teammateMessage(
    w,
    [
      {
        id: "team-lead",
        body: JSON.stringify({
          type: "task_assignment",
          taskId: "3",
          subject: "压测 Redis 实现的 p99",
          description: "用 oha 在 1k QPS 下压 30 秒，内存与 Redis 各一次。",
          assignedBy: "team-lead",
          timestamp: new Date(w.now + 5_000).toISOString(),
        }),
      },
    ],
    5_000,
  );
  // Two messages in one: a report and the end of the round with its result.
  teammateMessage(
    w,
    [
      {
        id: "bench-runner",
        color: "green",
        summary: "压测结果：Redis p99 高 3 ms",
        body: "压测跑完（oha，1k QPS，30 秒）：\n\n| 实现 | p50 | p99 | 429 比例 |\n|---|---|---|---|\n| memory | 1.8 ms | 4.1 ms | 0% |\n| redis | 2.6 ms | 7.2 ms | 0% |",
      },
      {
        id: "bench-runner",
        color: "green",
        body: JSON.stringify({
          type: "idle_notification",
          from: "bench-runner",
          timestamp: new Date(w.now + 900_000).toISOString(),
          idleReason: "available",
          result:
            "压测完成：Redis 实现 p99 7.2 ms，比内存版高约 3 ms，在 1k QPS 下没有触发 429。脚本在 scripts/bench.sh。",
        }),
      },
    ],
    900_000,
  );
  const t3 = w.now;
  w.call(
    "Edit",
    { file_path: f("config.toml"), old_string: 'backend = "memory"', new_string: 'backend = "redis"' },
    edit(f("config.toml"), 'backend = "memory"', 'backend = "redis"', [
      { oldStart: 4, oldLines: 1, newStart: 4, newLines: 1, lines: ['-backend = "memory"', '+backend = "redis"'] },
    ]),
  );
  w.say("两条线都完成了：Redis 计数 p99 多约 3 ms，可以接受，默认切到 Redis。我让两个 teammate 退出。", { end: true });
  w.endTurn(t3, 3);

  // The other session's message, then both teammates shut down (approval + system notice: one line each).
  peerMessage(
    w,
    "codeck-99",
    "lumen-api 的 CI 现在要求 Redis 服务，我在 .github/workflows/ci.yml 里加了 services.redis，你那边合并时注意冲突。",
    120_000,
  );
  for (const [name, color] of [
    ["redis-store", "blue"],
    ["bench-runner", "green"],
  ] as const) {
    teammateMessage(
      w,
      [
        {
          id: name,
          color,
          body: JSON.stringify({
            type: "shutdown_approved",
            requestId: `shutdown-${w.now}@${name}`,
            from: name,
            timestamp: new Date(w.now + 30_000).toISOString(),
            paneId: "in-process",
            backendType: "in-process",
          }),
        },
        { id: "system", body: JSON.stringify({ type: "teammate_terminated", message: `${name} has shut down.` }) },
      ],
      30_000,
    );
  }
  const t4 = w.now;
  w.say("收到，CI 的 Redis 服务和我的改动不冲突。两个 teammate 都已退出。", { end: true });
  w.endTurn(t4, 2);
  return s;
}
