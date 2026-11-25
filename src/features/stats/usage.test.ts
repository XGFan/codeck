import { describe, expect, it } from "vitest";
import type { DailyModelTokens } from "@/ipc/bindings";
import { buildUsageSeries, chooseUsageBucket, elapsedDays, inputTotals, modelCostBreakdown, mondayOf } from "./usage";

const DAY_MS = 86_400_000;

const row = (
  day: string,
  model: string,
  opts: Partial<Omit<DailyModelTokens, "day" | "model">> = {},
): DailyModelTokens => ({
  day,
  model,
  input: opts.input ?? 100,
  output: opts.output ?? 50,
  cacheRead: opts.cacheRead ?? 1000,
  cacheCreation: opts.cacheCreation ?? 200,
  cost: opts.cost ?? 1,
  requests: opts.requests ?? 1,
});

describe("chooseUsageBucket", () => {
  it("按跨度选择 1d / 1w / 1mo", () => {
    expect(chooseUsageBucket(7)).toBe("1d");
    expect(chooseUsageBucket(92)).toBe("1d");
    expect(chooseUsageBucket(93)).toBe("1w");
    expect(chooseUsageBucket(730)).toBe("1w");
    expect(chooseUsageBucket(731)).toBe("1mo");
  });
});

describe("elapsedDays", () => {
  it("有界范围取起点到 min(终点, 现在)，不足一天按一天", () => {
    const start = new Date("2026-11-23T14:00:00").getTime();
    const end = start + 7 * DAY_MS;
    expect(elapsedDays(start, end, [], start + 2.5 * DAY_MS)).toBeCloseTo(2.5, 5);
    expect(elapsedDays(start, end, [], start + 10 * DAY_MS)).toBe(7);
    expect(elapsedDays(start, end, [], start + 3_600_000)).toBe(1);
  });

  it("全部范围从有数据的第一天 0 点算到现在", () => {
    const now = new Date("2026-11-25T12:00:00").getTime();
    const daily = [row("2026-11-20", "claude-opus-4-7"), row("2026-11-24", "claude-opus-4-7")];
    expect(elapsedDays(null, null, daily, now)).toBeCloseTo(5.5, 5);
  });
});

describe("buildUsageSeries", () => {
  it("1d 补齐范围内所有自然日（含未来空日），并按模型族堆叠", () => {
    const fromMs = new Date("2026-11-23T14:00:00").getTime();
    const toMs = new Date("2026-11-30T14:00:00").getTime() - 1;
    const daily = [
      row("2026-11-23", "claude-fable-5-1", { cost: 10 }),
      row("2026-11-24", "claude-opus-4-7", { cost: 4 }),
      row("2026-11-24", "claude-opus-4-8", { cost: 6 }),
    ];
    const s = buildUsageSeries(daily, "cost", { fromMs, toMs }, new Date("2026-11-25T14:00:00").getTime());
    expect(s.bucket).toBe("1d");
    expect(s.items).toHaveLength(8); // Mon 23 .. Mon 30
    expect(s.families).toEqual(["fable", "opus"]);
    expect(s.items[0]!.byFamily.fable).toBe(10);
    expect(s.items[1]!.byFamily.opus).toBe(10);
    expect(s.items[7]!.byFamily.opus).toBe(0);
  });

  it("超过 92 天按周一对齐聚合为 1w", () => {
    expect(mondayOf("2026-11-25")).toBe("2026-11-23"); // Wed -> Mon
    expect(mondayOf("2026-11-29")).toBe("2026-11-23"); // Sun -> Mon
    const daily = [
      row("2026-08-01", "claude-opus-4-7", { output: 100 }),
      row("2026-11-24", "claude-opus-4-7", { output: 200 }),
      row("2026-11-25", "claude-opus-4-7", { output: 300 }),
    ];
    const s = buildUsageSeries(daily, "output", null, new Date("2026-11-25T14:00:00").getTime());
    expect(s.bucket).toBe("1w");
    const lastWeek = s.items[s.items.length - 1]!;
    expect(lastWeek.key).toBe("2026-11-23");
    expect(lastWeek.byFamily.opus).toBe(500);
  });

  it("超过 2 年聚合为 1mo，且异常旧记录最多只保留最近 170 根柱子", () => {
    const daily = [
      row("2024-01-10", "claude-opus-4-7", { input: 10, cacheRead: 20, cacheCreation: 5 }),
      row("2024-01-20", "claude-opus-4-7", { input: 5, cacheRead: 10, cacheCreation: 0 }),
      row("2026-05-01", "claude-fable-5-1", { input: 1, cacheRead: 0, cacheCreation: 0 }),
    ];
    const s = buildUsageSeries(daily, "input", null, new Date("2026-05-10T10:00:00").getTime());
    expect(s.bucket).toBe("1mo");
    expect(s.items[0]!.key).toBe("2024-01");
    expect(s.items[0]!.byFamily.opus).toBe(50);
    expect(s.items[s.items.length - 1]!.key).toBe("2026-05");

    const capped = buildUsageSeries(
      [row("1970-01-01", "claude-opus-4-7", { cost: 50 }), row("2026-11-25", "claude-fable-5-1", { cost: 10 })],
      "cost",
      null,
      new Date("2026-11-25T10:00:00").getTime(),
    );
    expect(capped.bucket).toBe("1mo");
    expect(capped.items).toHaveLength(170);
    expect(capped.items[capped.items.length - 1]!.key).toBe("2026-11");
    expect(capped.families).toEqual(["fable"]);
  });
});

describe("modelCostBreakdown & inputTotals", () => {
  it("按 Cost 倒序排列，合并同族的全部完整模型名", () => {
    const daily = [
      row("2026-11-23", "claude-opus-4-7", { cost: 30, input: 1, cacheRead: 10, cacheCreation: 100 }),
      row("2026-11-24", "claude-opus-4-8", { cost: 10, input: 2, cacheRead: 20, cacheCreation: 200 }),
      row("2026-11-24", "claude-fable-5-1", { cost: 60, input: 3, cacheRead: 30, cacheCreation: 300 }),
    ];
    expect(inputTotals(daily)).toEqual({ input: 6, read: 60, write: 600 });
    expect(modelCostBreakdown(daily)).toEqual([
      { family: "fable", cost: 60, shareExact: 60, sharePct: 60, models: ["claude-fable-5-1"] },
      { family: "opus", cost: 40, shareExact: 40, sharePct: 40, models: ["claude-opus-4-7", "claude-opus-4-8"] },
    ]);
  });
});
