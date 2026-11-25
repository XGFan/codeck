import { describe, expect, it } from "vitest";
import type { PlanLimit, PlanUsage } from "@/ipc/bindings";
import {
  buildMeterView,
  buildRingView,
  cycleRuleFrom,
  derivePlanState,
  formatAnchorReset,
  formatPlanBadge,
  formatResetAt,
  formatResetCountdown,
  formatUpdatedAt,
  resolvePlanAnchor,
  sanitizeAnchorMs,
} from "./model";

const sampleUsage: PlanUsage = {
  plan: "max",
  fetchedMs: new Date("2026-11-25T14:11:00").getTime(),
  limits: [
    { kind: "session", model: null, percent: 16.4, resetsMs: new Date("2026-11-25T16:40:00").getTime() },
    { kind: "weeklyAll", model: null, percent: 31, resetsMs: new Date("2026-11-30T14:00:00.789").getTime() },
    { kind: "weeklyScoped", model: "Fable", percent: 45, resetsMs: new Date("2026-11-30T14:00:00.789").getTime() },
  ],
};

describe("derivePlanState", () => {
  it("无结果无错误为 loading", () => {
    expect(derivePlanState({ data: undefined, error: null })).toEqual({ kind: "loading" });
  });

  it("从未成功且报错为 failed", () => {
    expect(derivePlanState({ data: undefined, error: { code: "io", message: "读取订阅额度超时" } })).toEqual({
      kind: "failed",
      error: "读取订阅额度超时",
    });
  });

  it("最近一次成功为 null 时判定为 none（即使后续重取报错）", () => {
    expect(derivePlanState({ data: null, error: null })).toEqual({ kind: "none" });
    expect(derivePlanState({ data: null, error: new Error("timeout") })).toEqual({ kind: "none" });
  });

  it("最近一次成功为 Some 时为 ok；后续重取失败仍保留旧数据并附带 error", () => {
    expect(derivePlanState({ data: sampleUsage, error: null })).toEqual({
      kind: "ok",
      usage: sampleUsage,
      error: null,
    });
    expect(derivePlanState({ data: sampleUsage, error: { code: "io", message: "未找到 claude" } })).toEqual({
      kind: "ok",
      usage: sampleUsage,
      error: "未找到 claude",
    });
  });
});

describe("formatPlanBadge & formatUpdatedAt", () => {
  it("套餐徽标首字母大写，空值返回 null", () => {
    expect(formatPlanBadge("max")).toBe("Max");
    expect(formatPlanBadge("pro")).toBe("Pro");
    expect(formatPlanBadge("  ")).toBeNull();
    expect(formatPlanBadge(null)).toBeNull();
  });

  it("更新时间在正常与带错误时给出对应文案", () => {
    const ms = new Date("2026-11-25T14:11:00").getTime();
    expect(formatUpdatedAt(ms, null)).toBe("14:11 更新");
    expect(formatUpdatedAt(ms, "超时")).toBe("读取失败，显示的是 14:11 的数据");
  });
});

describe("buildMeterView", () => {
  const nowMs = new Date("2026-11-25T13:28:00").getTime();

  it("落后于时间进度：不标记超前，strip 只写重置倒计时，popover 写重置时刻，bar 悬停提示时间已过", () => {
    const session: PlanLimit = {
      kind: "session",
      model: null,
      percent: 16.4,
      resetsMs: new Date("2026-11-25T16:40:00").getTime(), // 3h 12m later -> elapsed 108/300 = 36%
    };
    const strip = buildMeterView(session, nowMs, "strip");
    expect(strip).toMatchObject({
      label: "当前 5 小时",
      percent: 16,
      fillPct: 16.4,
      tone: "normal",
      isAhead: false,
      aheadStartPct: null,
      aheadByPct: null,
      elapsedTitle: "时间已过 36%",
      subtext: "3 小时 12 分后重置",
      aheadText: null,
      ariaLabel: "当前 5 小时已用 16%",
    });

    const pop = buildMeterView(session, nowMs, "popover");
    expect(pop).toMatchObject({
      isAhead: false,
      aheadStartPct: null,
      aheadByPct: null,
      elapsedTitle: "时间已过 36%",
      subtext: "3 小时 12 分后重置 · 今天 16:40",
      aheadText: null,
      ariaLabel: "当前 5 小时已用 16%",
    });
  });

  it("超前阈值：差 2.9 个百分点不算、差 3 个算；strip 带超前文案，popover 不带超前文案但进度条与 aria 标出超前", () => {
    // 5h window: 2h 30m remaining -> elapsed = 50% exactly.
    const halfElapsedReset = nowMs + 150 * 60_000;

    const under3 = buildMeterView(
      { kind: "session", model: null, percent: 52.9, resetsMs: halfElapsedReset },
      nowMs,
      "strip",
    );
    expect(under3).toMatchObject({
      percent: 53,
      fillPct: 52.9,
      isAhead: false,
      aheadStartPct: null,
      aheadByPct: null,
      elapsedTitle: "时间已过 50%",
      subtext: "2 小时 30 分后重置",
      aheadText: null,
      ariaLabel: "当前 5 小时已用 53%",
    });

    const at3 = buildMeterView(
      { kind: "session", model: null, percent: 53, resetsMs: halfElapsedReset },
      nowMs,
      "strip",
    );
    expect(at3).toMatchObject({
      percent: 53,
      fillPct: 53,
      isAhead: true,
      aheadStartPct: 50,
      aheadByPct: 3,
      elapsedTitle: "时间已过 50%",
      subtext: "2 小时 30 分后重置",
      aheadText: "超前 3%",
      ariaLabel: "当前 5 小时已用 53%，比时间进度多 3%",
    });

    // Weekly item ahead: strip shows aheadText, popover keeps reset wall-clock time without aheadText.
    // 5d remaining of 7d -> elapsed = 2/7 * 100 ≈ 28.5714%, 45 - 28.5714 = 16.4286 -> rounded 16%.
    const weekly: PlanLimit = {
      kind: "weeklyScoped",
      model: "Fable",
      percent: 45,
      resetsMs: new Date("2026-11-30T13:59:59.800").getTime(),
    };
    const weeklyStrip = buildMeterView(weekly, nowMs, "strip");
    expect(weeklyStrip.isAhead).toBe(true);
    expect(weeklyStrip.aheadStartPct).toBeCloseTo(((7 * 24 - 120.5333) / (7 * 24)) * 100, 1);
    expect(weeklyStrip.aheadByPct).toBe(17);
    expect(weeklyStrip.aheadText).toBe("超前 17%");
    expect(weeklyStrip.ariaLabel).toBe("本周 · Fable已用 45%，比时间进度多 17%");

    const pop = buildMeterView(weekly, nowMs, "popover");
    expect(pop.label).toBe("本周 · Fable");
    expect(pop.isAhead).toBe(true);
    expect(pop.aheadText).toBeNull();
    expect(pop.subtext).toBe("5 天后重置 · 11月30日 周一 14:00");
    expect(pop.ariaLabel).toBe("本周 · Fable已用 45%，比时间进度多 17%");
  });

  it("≥80% 琥珀色写「快用完了」，≥100% 红色写「已用完」且用量按 100 封顶计算超前；阈值按原始百分比判定", () => {
    // 48 min left of 300 min -> elapsed = 84%.
    const under80: PlanLimit = { kind: "session", model: null, percent: 79.5, resetsMs: nowMs + 48 * 60_000 };
    expect(buildMeterView(under80, nowMs, "strip")).toMatchObject({
      percent: 80,
      tone: "normal",
      isAhead: false,
      elapsedTitle: "时间已过 84%",
      subtext: "48 分后重置",
      aheadText: null,
      ariaLabel: "当前 5 小时已用 80%",
    });

    const at80: PlanLimit = { kind: "session", model: null, percent: 80, resetsMs: nowMs + 48 * 60_000 };
    expect(buildMeterView(at80, nowMs, "strip")).toMatchObject({
      percent: 80,
      tone: "warn",
      isAhead: false,
      subtext: "快用完了 · 48 分后重置",
      aheadText: null,
    });

    const under100: PlanLimit = { kind: "session", model: null, percent: 99.5, resetsMs: nowMs + 48 * 60_000 };
    expect(buildMeterView(under100, nowMs, "strip")).toMatchObject({
      percent: 100,
      fillPct: 99.5,
      tone: "warn",
      isAhead: true,
      aheadStartPct: 84,
      aheadByPct: 16,
      subtext: "快用完了 · 48 分后重置",
      aheadText: "超前 16%",
      ariaLabel: "当前 5 小时已用 100%，比时间进度多 16%",
    });

    // 125 min left of 300 min -> elapsed = 175/300 * 100 = 58.3333%.
    const at100: PlanLimit = {
      kind: "session",
      model: null,
      percent: 100,
      resetsMs: nowMs + (2 * 60 + 5) * 60_000,
    };
    expect(buildMeterView(at100, nowMs, "strip")).toMatchObject({
      percent: 100,
      fillPct: 100,
      tone: "full",
      isAhead: true,
      aheadByPct: 42,
      elapsedTitle: "时间已过 58%",
      subtext: "已用完 · 2 小时 5 分后重置",
      aheadText: "超前 42%",
    });

    // Over 100% clamps usage to 100 when computing ahead diff: 103.2 clamped to 100, 100 - 98 = 2 < 3 -> not ahead.
    const over100NearEnd: PlanLimit = {
      kind: "session",
      model: null,
      percent: 103.2,
      resetsMs: nowMs + 6 * 60_000, // elapsed = 294/300 = 98%
    };
    expect(buildMeterView(over100NearEnd, nowMs, "strip")).toMatchObject({
      percent: 103,
      fillPct: 100,
      tone: "full",
      isAhead: false,
      aheadStartPct: null,
      aheadByPct: null,
      aheadText: null,
    });
  });

  it("没有 resetsMs 或已过去：不算超前、无时间提示、不写重置时间", () => {
    const noReset: PlanLimit = { kind: "weeklyAll", model: null, percent: 90, resetsMs: null };
    expect(buildMeterView(noReset, nowMs, "strip")).toMatchObject({
      label: "本周 · 全部模型",
      tone: "warn",
      isAhead: false,
      aheadStartPct: null,
      aheadByPct: null,
      elapsedTitle: null,
      subtext: "快用完了",
      aheadText: null,
      ariaLabel: "本周 · 全部模型已用 90%",
    });

    const past: PlanLimit = { kind: "session", model: null, percent: 85, resetsMs: nowMs - 1000 };
    expect(buildMeterView(past, nowMs, "strip")).toMatchObject({
      isAhead: false,
      aheadStartPct: null,
      aheadByPct: null,
      elapsedTitle: null,
      subtext: "快用完了",
      aheadText: null,
      ariaLabel: "当前 5 小时已用 85%",
    });

    const exactNow: PlanLimit = { kind: "session", model: null, percent: 100, resetsMs: nowMs };
    expect(buildMeterView(exactNow, nowMs, "popover")).toMatchObject({
      isAhead: false,
      elapsedTitle: null,
      subtext: "已用完",
    });
  });

  it("时间比例夹在 0–100%（远期重置时间已过为 0%）", () => {
    const farFuture: PlanLimit = { kind: "session", model: null, percent: 10, resetsMs: nowMs + 10 * 3_600_000 };
    expect(buildMeterView(farFuture, nowMs, "strip")).toMatchObject({
      isAhead: true,
      aheadStartPct: 0,
      aheadByPct: 10,
      elapsedTitle: "时间已过 0%",
      aheadText: "超前 10%",
    });
  });
});

describe("buildRingView", () => {
  it("取各项中最高的百分比并判定色调与弧长", () => {
    const ring = buildRingView(sampleUsage);
    expect(ring.percent).toBe(45);
    expect(ring.tone).toBe("normal");
    expect(ring.ariaLabel).toBe("额度：最高一项已用 45%");

    const warn = buildRingView({
      ...sampleUsage,
      limits: [{ kind: "session", model: null, percent: 82.4, resetsMs: null }],
    });
    expect(warn).toMatchObject({ percent: 82, tone: "warn", ariaLabel: "额度：最高一项已用 82%" });

    const full = buildRingView({
      ...sampleUsage,
      limits: [{ kind: "weeklyAll", model: null, percent: 105, resetsMs: null }],
    });
    expect(full).toMatchObject({ percent: 105, tone: "full", ariaLabel: "额度：最高一项已用 105%" });

    // Raw threshold check on ring: 79.5 -> normal (displays 80%), 99.5 -> warn (displays 100%).
    expect(
      buildRingView({ ...sampleUsage, limits: [{ kind: "session", model: null, percent: 79.5, resetsMs: null }] }),
    ).toMatchObject({ percent: 80, tone: "normal", ariaLabel: "额度：最高一项已用 80%" });
    expect(
      buildRingView({ ...sampleUsage, limits: [{ kind: "session", model: null, percent: 99.5, resetsMs: null }] }),
    ).toMatchObject({ percent: 100, tone: "warn", ariaLabel: "额度：最高一项已用 100%" });
  });
});

describe("resolvePlanAnchor & cycleRuleFrom & formatAnchorReset", () => {
  const weekly = { weekday: 0, minutes: 14 * 60 };
  const saved = new Date("2026-11-23T14:00:00").getTime();

  it("ok 且有 weeklyAll.resetsMs 时取整到分钟作锚，相差几百毫秒跨整分也得到同一个锚", () => {
    const res = resolvePlanAnchor({ kind: "ok", usage: sampleUsage, error: null }, saved);
    const expected = new Date("2026-11-30T14:00:00.000").getTime();
    expect(res).toEqual({ nextSaved: expected, anchorMs: expected });
    expect(cycleRuleFrom(res.anchorMs, weekly)).toEqual({ kind: "anchor", anchorMs: expected });

    const beforeMin: PlanUsage = {
      ...sampleUsage,
      limits: [{ kind: "weeklyAll", model: null, percent: 31, resetsMs: Date.parse("2026-11-30T05:59:59.698Z") }],
    };
    const afterMin: PlanUsage = {
      ...sampleUsage,
      limits: [{ kind: "weeklyAll", model: null, percent: 31, resetsMs: Date.parse("2026-11-30T06:00:00.238Z") }],
    };
    const a1 = resolvePlanAnchor({ kind: "ok", usage: beforeMin, error: null }, null).anchorMs;
    const a2 = resolvePlanAnchor({ kind: "ok", usage: afterMin, error: null }, null).anchorMs;
    expect(a1).toBe(Date.parse("2026-11-30T06:00:00.000Z"));
    expect(a2).toBe(a1);
  });

  it("ok 但无 weeklyAll.resetsMs 时保持已存值不动", () => {
    const noWeekly: PlanUsage = {
      plan: "max",
      fetchedMs: Date.now(),
      limits: [{ kind: "session", model: null, percent: 20, resetsMs: Date.now() + 60_000 }],
    };
    expect(resolvePlanAnchor({ kind: "ok", usage: noWeekly, error: null }, saved)).toEqual({
      nextSaved: saved,
      anchorMs: saved,
    });
  });

  it("none 清掉已存值并回退设置", () => {
    const res = resolvePlanAnchor({ kind: "none" }, saved);
    expect(res).toEqual({ nextSaved: null, anchorMs: null });
    expect(cycleRuleFrom(res.anchorMs, weekly)).toEqual({ kind: "weekly", reset: weekly });
  });

  it("loading / failed 沿用已存值，没有则回退设置", () => {
    expect(resolvePlanAnchor({ kind: "loading" }, saved)).toEqual({ nextSaved: saved, anchorMs: saved });
    expect(resolvePlanAnchor({ kind: "failed", error: "超时" }, saved)).toEqual({ nextSaved: saved, anchorMs: saved });
    expect(resolvePlanAnchor({ kind: "failed", error: "超时" }, null)).toEqual({ nextSaved: null, anchorMs: null });
  });

  it("不合法的锚（NaN、Infinity、越界、负数、非数字字符串）被视为无效并清掉", () => {
    expect(sanitizeAnchorMs(Number.NaN)).toBeNull();
    expect(sanitizeAnchorMs(Number.POSITIVE_INFINITY)).toBeNull();
    expect(sanitizeAnchorMs(Number.NEGATIVE_INFINITY)).toBeNull();
    expect(sanitizeAnchorMs(1e20)).toBeNull();
    expect(sanitizeAnchorMs(-1000)).toBeNull();
    expect(sanitizeAnchorMs("not-a-number")).toBeNull();
    expect(sanitizeAnchorMs("")).toBeNull();
    expect(sanitizeAnchorMs(Date.UTC(2019, 11, 31))).toBeNull();
    expect(sanitizeAnchorMs(Date.UTC(2100, 0, 2))).toBeNull();

    const valid = Date.UTC(2026, 10, 30, 6, 0, 0);
    expect(sanitizeAnchorMs(String(valid))).toBe(valid);

    // Invalid saved anchor is cleared during loading/failed too.
    expect(resolvePlanAnchor({ kind: "loading" }, Number.NaN)).toEqual({ nextSaved: null, anchorMs: null });
    expect(resolvePlanAnchor({ kind: "failed", error: "超时" }, 1e20)).toEqual({ nextSaved: null, anchorMs: null });
  });

  it("formatAnchorReset 与 formatResetAt / formatResetCountdown 符合文案规范", () => {
    expect(formatAnchorReset(new Date("2026-11-30T13:59:59.789").getTime())).toBe("周一 14:00");
    expect(formatResetAt(new Date("2026-11-25T16:40:20").getTime(), new Date("2026-11-25T13:00:00").getTime())).toBe(
      "今天 16:40",
    );
    expect(formatResetCountdown(48 * 60_000)).toBe("48 分");
    expect(formatResetCountdown(20_000)).toBe("1 分");
    expect(formatResetCountdown(3 * 3_600_000)).toBe("3 小时");
  });
});
