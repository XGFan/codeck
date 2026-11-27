import { describe, expect, it } from "vitest";
import type { HaaWorker } from "@/ipc/protocol/haa";
import {
  formatCollapsedSummary,
  formatTaskText,
  formatTaskTooltip,
  formatWorkerDuration,
  isReadById,
  readsWorkerById,
  shortModelName,
  sortWorkers,
  workerBackend,
} from "./view";

const NOW = 1_700_000_000_000;
const w = (over: Partial<HaaWorker> & Pick<HaaWorker, "state">): HaaWorker => ({
  id: "w-1",
  owner: "s-1",
  name: "alpha",
  tokens: 0,
  ...over,
});

describe("formatWorkerDuration", () => {
  it("formats running and asking from startedAt with sub-second clamped to 0 秒", () => {
    expect(formatWorkerDuration(w({ state: "running", startedAt: NOW - 450 }), NOW)).toBe("0 秒");
    expect(formatWorkerDuration(w({ state: "running", startedAt: NOW + 500 }), NOW)).toBe("0 秒");
    expect(formatWorkerDuration(w({ state: "running", startedAt: NOW - 15_000 }), NOW)).toBe("15 秒");
    expect(formatWorkerDuration(w({ state: "asking", startedAt: NOW - 64_000 }), NOW)).toBe("1 分 04 秒");
    expect(formatWorkerDuration(w({ state: "running", startedAt: NOW - 3_660_000 }), NOW)).toBe("1 小时 1 分");
    expect(formatWorkerDuration(w({ state: "running", lastActive: NOW - 5_000 }), NOW)).toBe("—");
  });

  it("formats non-running/asking states from lastActive with floor thresholds", () => {
    expect(formatWorkerDuration(w({ state: "idle", lastActive: NOW - 59_999 }), NOW)).toBe("刚刚");
    expect(formatWorkerDuration(w({ state: "taken", lastActive: NOW - 60_000 }), NOW)).toBe("1 分钟");
    expect(formatWorkerDuration(w({ state: "idle", lastActive: NOW - 3_599_999 }), NOW)).toBe("59 分钟");
    expect(formatWorkerDuration(w({ state: "dormant", lastActive: NOW - 3_600_000 }), NOW)).toBe("1 小时");
    expect(formatWorkerDuration(w({ state: "dormant", lastActive: NOW - 86_399_999 }), NOW)).toBe("23 小时");
    expect(formatWorkerDuration(w({ state: "dead", lastActive: NOW - 86_400_000 }), NOW)).toBe("1 天");
    expect(formatWorkerDuration(w({ state: "dead", lastActive: NOW - 3 * 86_400_000 - 40_000 }), NOW)).toBe("3 天");
    expect(formatWorkerDuration(w({ state: "idle", startedAt: NOW - 5_000 }), NOW)).toBe("—");
  });
});

describe("sortWorkers", () => {
  it("sorts by state order and then by duration ascending with missing timestamps last", () => {
    const input: HaaWorker[] = [
      w({ id: "dead", state: "dead", lastActive: NOW - 1_000 }),
      w({ id: "idle-missing", state: "idle" }),
      w({ id: "idle-long", state: "idle", lastActive: NOW - 120_000 }),
      w({ id: "idle-short", state: "idle", lastActive: NOW - 10_000 }),
      w({ id: "dormant", state: "dormant", lastActive: NOW - 5_000 }),
      w({ id: "taken", state: "taken", lastActive: NOW - 5_000 }),
      w({ id: "asking", state: "asking", startedAt: NOW - 5_000 }),
      w({ id: "run-long", state: "running", startedAt: NOW - 30_000 }),
      w({ id: "run-short", state: "running", startedAt: NOW - 2_000 }),
    ];
    const sorted = sortWorkers(input, NOW);
    expect(sorted.map((x) => x.id)).toEqual([
      "run-short",
      "run-long",
      "asking",
      "taken",
      "idle-short",
      "idle-long",
      "idle-missing",
      "dormant",
      "dead",
    ]);
    expect(input[0]!.id).toBe("dead");
  });
});

describe("shortModelName", () => {
  it("extracts the segment after the last slash", () => {
    expect(shortModelName("agy/gemini-4-neon-high")).toBe("gemini-4-neon-high");
    expect(shortModelName("codex/gpt-6.2-sol:high")).toBe("gpt-6.2-sol:high");
    expect(shortModelName("agent-gateway/deepseek-v4.1-flash")).toBe("deepseek-v4.1-flash");
    expect(shortModelName("a/b/c")).toBe("c");
    expect(shortModelName("plain")).toBe("plain");
    expect(shortModelName(undefined)).toBe("");
  });
});

describe("workerBackend and readsWorkerById", () => {
  it("reads the backend before the first slash; codex and pi Workers are read by id", () => {
    expect(workerBackend("pi/agent-gateway/deepseek-v4.1-flash")).toBe("pi");
    expect(workerBackend("gpt-6.2-sol")).toBeNull();
    expect(workerBackend(undefined)).toBeNull();
    expect(readsWorkerById("codex/gpt-6.2-sol:high")).toBe(true);
    expect(readsWorkerById("pi/agent-gateway/deepseek-v4.1-flash")).toBe(true);
    expect(readsWorkerById("agy/gemini-4-neon-high")).toBe(false);
    // Older haa wrote the model without its backend.
    expect(readsWorkerById("agent-gateway/deepseek-v4.1-flash")).toBe(false);
    expect([isReadById("codex"), isReadById("pi"), isReadById("agy"), isReadById(null)]).toEqual([
      true,
      true,
      false,
      false,
    ]);
  });
});

describe("formatTaskText and formatTaskTooltip", () => {
  it("handles asking with and without pending", () => {
    const asking = w({
      state: "asking",
      title: "审查 PR",
      pending: "第一行问题\n  第二行细节\t第三段",
    });
    expect(formatTaskText(asking)).toBe("问 Main：第一行问题 第二行细节 第三段");
    expect(formatTaskTooltip(asking)).toBe("问 Main：第一行问题\n  第二行细节\t第三段");

    const askingNoPending = w({ state: "asking", task: "回退任务" });
    expect(formatTaskText(askingNoPending)).toBe("回退任务");
    expect(formatTaskTooltip(askingNoPending)).toBe("回退任务");
  });

  it("handles running with title/task fallback and activity only in tooltip", () => {
    const withTitle = w({ state: "running", title: "修复限流", task: "原任务", activity: "cargo test" });
    expect(formatTaskText(withTitle)).toBe("修复限流");
    expect(formatTaskTooltip(withTitle)).toBe("修复限流\n正在做：cargo test");

    const taskOnly = w({ state: "running", task: "跑构建" });
    expect(formatTaskText(taskOnly)).toBe("跑构建");
    expect(formatTaskTooltip(taskOnly)).toBe("跑构建");
  });

  it("handles taken and inactive states", () => {
    const taken = w({ state: "taken", title: "手动排查", activity: "ignored" });
    expect(formatTaskText(taken)).toBe("手动排查");
    expect(formatTaskTooltip(taken)).toBe("手动排查");

    const idle = w({ state: "idle", tasks: 3, title: "旧标题" });
    expect(formatTaskText(idle)).toBe("做过 3 个 Task");
    expect(formatTaskTooltip(idle)).toBe("做过 3 个 Task");

    const dormantZero = w({ state: "dormant", tasks: 0 });
    expect(formatTaskText(dormantZero)).toBe("做过 0 个 Task");

    const deadNoTasks = w({ state: "dead" });
    expect(formatTaskText(deadNoTasks)).toBe("");
    expect(formatTaskTooltip(deadNoTasks)).toBe("");
  });
});

describe("formatCollapsedSummary", () => {
  it("joins non-zero inactive counts in idle, dormant, dead order", () => {
    expect(
      formatCollapsedSummary([
        w({ state: "dormant" }),
        w({ state: "idle" }),
        w({ state: "idle" }),
        w({ state: "dormant" }),
        w({ state: "dormant" }),
        w({ state: "dormant" }),
        w({ state: "idle" }),
      ]),
    ).toBe("3 个空闲 · 4 个休眠");

    expect(formatCollapsedSummary([w({ state: "dead" }), w({ state: "running" }), w({ state: "idle" })])).toBe(
      "1 个空闲 · 1 个已退出",
    );

    expect(formatCollapsedSummary([w({ state: "running" })])).toBe("");
  });
});
