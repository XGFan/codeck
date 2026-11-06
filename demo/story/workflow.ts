// Workflow Session：按维度复核改动并逐条核实（两阶段，一个 agent 失败）。

import { workflow, workflowNotification } from "../../scripts/demo/agents.ts";
import { bash, prompt, read } from "../../scripts/demo/kit.ts";
import { Session } from "../../scripts/demo/lib.ts";

const f = (p: string) => `/Users/dev/code/lumen-api/${p}`;

export function workflowSession(): Session {
  const s = new Session({
    id: "c7a2e915-6d4b-48f3-b2e0-81f5a9c3d6e4",
    project: "lumen-api",
    branch: "feat/rate-limit",
    start: "2026-11-05T06:10:00.000Z",
    title: "复核限流改动",
    slug: "review-rate-limit",
  });
  const w = s.main;
  prompt(w, "用 review-changes 复核这个分支的限流改动。", 60_000);
  const t0 = w.now;
  const run = workflow(w, {
    name: "review-changes",
    summary: "按维度复核改动并逐条核实",
    args: { base: "main", dimensions: ["bugs", "perf"] },
    phases: [
      { title: "Review", detail: "按维度找问题" },
      { title: "Verify", detail: "逐条核实" },
    ],
    agents: [
      {
        label: "review:bugs",
        phase: 1,
        prompt: "找 feat/rate-limit 相对 main 的正确性问题。",
        run: (sub) => {
          sub.call(
            "Bash",
            { command: "git diff main --stat", description: "看改动" },
            bash(" src/middleware/ratelimit.rs | 48 ++++\n src/main.rs | 1 +", { ms: 300 }),
          );
          return "1 处：check() 在时钟回拨时 elapsed 会 panic。";
        },
      },
      {
        label: "review:perf",
        phase: 1,
        prompt: "找性能问题。",
        run: (sub) => {
          sub.call(
            "Read",
            { file_path: f("src/middleware/ratelimit.rs") },
            read(f("src/middleware/ratelimit.rs"), "let mut map = self.buckets.lock();"),
          );
          return "整张表一把锁，1k QPS 下争用可接受；建议换 DashMap。";
        },
      },
      {
        label: "verify:ratelimit.rs",
        phase: 2,
        prompt: "核实：时钟回拨时 elapsed 会 panic。",
        run: () => "属实：Instant 单调，不会回拨，但 elapsed 在 last 未来时会饱和为 0，不会 panic；降级为提示。",
      },
      {
        label: "verify:main.rs",
        phase: 2,
        failed: true,
        prompt: "核实：限流层顺序。",
        run: () => "API Error: Connection dropped (ECONNRESET)",
      },
    ],
    result: { findings: 2, confirmed: 1, failed: 1 },
  });
  w.say("Workflow 在后台跑，完成后汇总。", { end: true });
  w.endTurn(t0, 2);
  workflowNotification(w, run, 30_000);
  const t1 = w.now;
  w.say(
    "复核完成：2 条发现，核实 1 条（DashMap 的建议），时钟回拨那条不成立；核实 main.rs 的 agent 因网络错误失败，我手动看过，限流层在 auth 之后，没问题。",
    { end: true },
  );
  w.endTurn(t1, 2);
  return s;
}
