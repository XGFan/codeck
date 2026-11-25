import { describe, expect, it } from "vitest";
import type { HourUsage } from "@/ipc/bindings";
import type { PlanState } from "@/features/quota";
import {
  buildCumulativeSeries,
  computeQuotaEstimate,
  computeTrendSummary,
  trendLabels,
  trendYAxis,
  trendYAxisMax,
  usablePrevStats,
  valueAtElapsed,
} from "./trend";

const H = 3_600_000;

describe("buildCumulativeSeries & valueAtElapsed", () => {
  it("逐桶累加且只升不降，进行中截断在 now，同一时刻按桶内线性插值", () => {
    const start = new Date("2026-11-23T14:00:00").getTime();
    const end = start + 4 * H;
    const hourly: HourUsage[] = [
      { startMs: start, endMs: start + H, requests: 1, cost: 10 },
      { startMs: start + H, endMs: start + 2 * H, requests: 2, cost: 20 },
      { startMs: start + 2 * H, endMs: start + 3 * H, requests: 1, cost: 30 },
      { startMs: start + 3 * H, endMs: start + 4 * H, requests: 0, cost: 0 },
    ];

    const full = buildCumulativeSeries(hourly, start, end);
    expect(full).toEqual([
      [0, 0],
      [H, 10],
      [2 * H, 30],
      [3 * H, 60],
      [4 * H, 60],
    ]);
    // 1.5h elapsed -> midpoint between (1h, 10) and (2h, 30) = 20.
    expect(valueAtElapsed(full, 1.5 * H)).toBe(20);

    // Cutoff at 1.5h: includes bucket 0 + partial bucket 1, stops at 1.5h.
    const partial = buildCumulativeSeries(hourly, start, end, start + 1.5 * H);
    expect(partial).toEqual([
      [0, 0],
      [H, 10],
      [1.5 * H, 30],
    ]);
  });
});

describe("computeTrendSummary", () => {
  const start = new Date("2026-11-23T14:00:00").getTime();
  const end = start + 7 * 24 * H;

  it("进行中按同一时刻比较，已结束按总额比较；占位数据或无前一时段时不写差额", () => {
    const cur = [
      [0, 0],
      [2 * H, 50],
    ] as [number, number][];
    const prev = [
      [0, 0],
      [4 * H, 160],
    ] as [number, number][]; // at 2h -> 80

    const live = computeTrendSummary({
      sel: { kind: "cycle", offset: 0 },
      rangeStartMs: start,
      rangeEndMs: end,
      nowMs: start + 2 * H,
      currentPts: cur,
      prevPts: prev,
      isPlaceholderData: false,
    });
    expect(live.inProgress).toBe(true);
    expect(live.subtitle).toBe("到现在 $50.00 · 比上一周期同一时刻少 $30.00");

    const placeholder = computeTrendSummary({
      sel: { kind: "cycle", offset: 0 },
      rangeStartMs: start,
      rangeEndMs: end,
      nowMs: start + 2 * H,
      currentPts: cur,
      prevPts: prev,
      isPlaceholderData: true,
    });
    expect(placeholder.diff).toBeNull();
    expect(placeholder.subtitle).toBe("到现在 $50.00");

    const ended = computeTrendSummary({
      sel: { kind: "cycle", offset: -1 },
      rangeStartMs: start,
      rangeEndMs: end,
      nowMs: end + H,
      currentPts: [
        [0, 0],
        [end - start, 200],
      ],
      prevPts: [
        [0, 0],
        [end - start, 150],
      ],
      isPlaceholderData: false,
    });
    expect(ended.inProgress).toBe(false);
    expect(ended.subtitle).toBe("共 $200.00 · 比前一周期多 $50.00");
  });

  it("单日走势：已结束与今天分别按全天/同一时刻比较前一天或上周期同一天，图例附日期", () => {
    const dayStart = new Date("2026-11-24T00:00:00").getTime();
    const dayEnd = new Date("2026-11-25T00:00:00").getTime();
    const prevDayStart = new Date("2026-11-23T00:00:00").getTime();
    const prevWeekStart = new Date("2026-11-17T00:00:00").getTime();

    expect(trendLabels({ kind: "day", offset: -1 }, "prevDay", prevDayStart)).toEqual({
      current: "这一天",
      previous: "前一天",
      previousLegend: "前一天（11月23日 周一）",
    });
    expect(trendLabels({ kind: "day", offset: -1 }, "prevWeek", prevWeekStart)).toEqual({
      current: "这一天",
      previous: "上周期同一天",
      previousLegend: "上周期同一天（11月17日 周二）",
    });

    const cur = [
      [0, 0],
      [24 * H, 143.6],
    ] as [number, number][];
    const prevDayPts = [
      [0, 0],
      [24 * H, 111.56],
    ] as [number, number][];
    const prevWeekPts = [
      [0, 0],
      [24 * H, 173.63],
    ] as [number, number][];

    const vsPrevDay = computeTrendSummary({
      sel: { kind: "day", offset: -1 },
      rangeStartMs: dayStart,
      rangeEndMs: dayEnd,
      nowMs: dayEnd + 14 * H,
      currentPts: cur,
      prevPts: prevDayPts,
      isPlaceholderData: false,
      dayCompare: "prevDay",
    });
    expect(vsPrevDay.subtitle).toBe("共 $143.60 · 比前一天多 $32.04");

    const vsPrevWeek = computeTrendSummary({
      sel: { kind: "day", offset: -1 },
      rangeStartMs: dayStart,
      rangeEndMs: dayEnd,
      nowMs: dayEnd + 14 * H,
      currentPts: cur,
      prevPts: prevWeekPts,
      isPlaceholderData: false,
      dayCompare: "prevWeek",
    });
    expect(vsPrevWeek.subtitle).toBe("共 $143.60 · 比上周期同一天少 $30.03");

    const noCompare = computeTrendSummary({
      sel: { kind: "day", offset: -1 },
      rangeStartMs: dayStart,
      rangeEndMs: dayEnd,
      nowMs: dayEnd + 14 * H,
      currentPts: cur,
      prevPts: null,
      isPlaceholderData: false,
      dayCompare: "prevDay",
    });
    expect(noCompare.subtitle).toBe("共 $143.60");

    // Today in progress at 12:00: linear interpolation against compare day.
    const todayLive = computeTrendSummary({
      sel: { kind: "day", offset: 0 },
      rangeStartMs: dayStart,
      rangeEndMs: dayEnd,
      nowMs: dayStart + 12 * H,
      currentPts: [
        [0, 0],
        [12 * H, 60],
      ],
      prevPts: [
        [0, 0],
        [24 * H, 100], // at 12h -> 50
      ],
      isPlaceholderData: false,
      dayCompare: "prevWeek",
    });
    expect(todayLive.inProgress).toBe(true);
    expect(todayLive.subtitle).toBe("到现在 $60.00 · 比上周期同一天同一时刻多 $10.00");
  });
});

describe("computeQuotaEstimate", () => {
  const nowMs = new Date("2026-11-25T14:11:00").getTime();
  const okPlan = (percent: number, resetsMs: number | null = nowMs + 3 * 24 * H): PlanState => ({
    kind: "ok",
    error: null,
    usage: {
      plan: "max",
      fetchedMs: nowMs,
      limits: [{ kind: "weeklyAll", model: null, percent, resetsMs }],
    },
  });

  it("满足全部条件时返回预估额度与图例", () => {
    const est = computeQuotaEstimate({
      sel: { kind: "cycle", offset: 0 },
      planState: okPlan(35),
      usedCost: 284.96,
      nowMs,
      isPlaceholderData: false,
    });
    expect(est).not.toBeNull();
    expect(est!.estimatedCost).toBeCloseTo(284.96 / 0.35, 2);
    expect(est!.lineLabel).toBe("预估额度 $814.17");
    expect(est!.legendText).toBe("预估额度 = 已用 Cost ÷ 本周已用 35%");
  });

  it("非本周期、占位中、非 ok、已过重置、百分比 < 5% 或已用 Cost <= 0 时不显示", () => {
    const base = {
      sel: { kind: "cycle", offset: 0 } as const,
      planState: okPlan(35),
      usedCost: 100,
      nowMs,
      isPlaceholderData: false,
    };
    expect(computeQuotaEstimate({ ...base, sel: { kind: "cycle", offset: -1 } })).toBeNull();
    expect(computeQuotaEstimate({ ...base, sel: { kind: "day", offset: 0 } })).toBeNull();
    expect(computeQuotaEstimate({ ...base, sel: { kind: "last7", offset: 0 } })).toBeNull();
    expect(computeQuotaEstimate({ ...base, isPlaceholderData: true })).toBeNull();
    expect(computeQuotaEstimate({ ...base, planState: { kind: "none" } })).toBeNull();
    expect(computeQuotaEstimate({ ...base, planState: okPlan(35, nowMs - 1000) })).toBeNull();
    expect(computeQuotaEstimate({ ...base, planState: okPlan(4.9) })).toBeNull();
    expect(computeQuotaEstimate({ ...base, usedCost: 0 })).toBeNull();
  });
});

describe("trendYAxisMax", () => {
  it("纵轴上限严格高于最大值（含预估额度）并向上取整为整齐刻度", () => {
    expect(trendYAxisMax(0)).toBe(1);
    expect(trendYAxisMax(275.03)).toBe(400);
    expect(trendYAxisMax(814.17)).toBe(1000);
    // The ceiling stays close to the data: $926 gets $1250 in $250 steps, not $1500.
    expect(trendYAxis(926.06)).toEqual({ max: 1250, step: 250 });
    expect(trendYAxis(275.03)).toEqual({ max: 400, step: 100 });
    expect(trendYAxis(5400)).toEqual({ max: 7500, step: 2500 });
    for (const p of [0.3, 12, 85, 275.03, 926.06, 5400]) {
      expect(trendYAxisMax(p)).toBeGreaterThanOrEqual(p * 1.12);
    }
  });
});

describe("usablePrevStats", () => {
  const sample = {
    overview: { outputTokens: 0, cost: 10, unpricedMessages: 0, requests: 1 },
    daily: [],
    hourly: [],
    hourlyModels: [],
    sessions: [],
  };

  it("仅在 status 为 success 且无错误时返回参照数据，后台重取失败即使保留旧 data 也忽略", () => {
    expect(usablePrevStats({ data: sample, status: "success", error: null })).toBe(sample);
    expect(usablePrevStats({ data: sample, status: "success", error: new Error("timeout") })).toBeUndefined();
    expect(usablePrevStats({ data: undefined, status: "pending", error: null })).toBeUndefined();
    expect(usablePrevStats({ data: undefined, status: "error", error: new Error("fail") })).toBeUndefined();
  });
});
