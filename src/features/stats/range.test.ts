import { describe, expect, it } from "vitest";
import { WEEK_MS, type CycleRule } from "@/lib/cycle";
import { dayOffset, pageable, resolveDayCompareRange, resolvePrev7Range, resolveRange } from "./range";

// A Thursday morning; the cycle resets on Mondays at 14:00.
const now = new Date("2026-10-15T10:03:27");
const weeklyRule: CycleRule = { kind: "weekly", reset: { weekday: 0, minutes: 14 * 60 } };
const at = (s: string) => new Date(s).getTime();

describe("resolveRange", () => {
  it("周期（weekly）：不论是否进行中，toMs 均为终点 - 1 且相邻周期不重叠", () => {
    const cur = resolveRange({ kind: "cycle", offset: 0 }, now, weeklyRule);
    expect(cur.timeRange).toEqual({ fromMs: at("2026-10-12T14:00:00"), toMs: at("2026-10-19T14:00:00") - 1 });
    expect(cur.label).toBe("本周期");
    expect(cur.detail).toBe("10月12日 周一 14:00 至 10月19日 周一 14:00，还剩 4 天 3 小时");

    const prev = resolveRange({ kind: "cycle", offset: -1 }, now, weeklyRule);
    expect(prev.timeRange).toEqual({ fromMs: at("2026-10-05T14:00:00"), toMs: at("2026-10-12T14:00:00") - 1 });
    expect(prev.label).toBe("上一周期");
    expect(prev.detail).toBe("10月5日 周一 14:00 至 10月12日 周一 14:00");

    expect(resolveRange({ kind: "cycle", offset: -3 }, now, weeklyRule).label).toBe("9月21日起的周期");
  });

  it("周期（anchor）：带毫秒不舍入，显示时四舍五入到分钟并附带跟随订阅说明", () => {
    // 13:59:59.800 rounds to 14:00 for display while keeping exact ms for timeRange.
    const anchorMs = at("2026-10-19T13:59:59.800");
    const rule: CycleRule = { kind: "anchor", anchorMs };
    const cur = resolveRange({ kind: "cycle", offset: 0 }, new Date("2026-10-15T10:00:00"), rule);
    expect(cur.timeRange).toEqual({ fromMs: anchorMs - WEEK_MS, toMs: anchorMs - 1 });
    expect(cur.detail).toBe("10月12日 周一 14:00 至 10月19日 周一 14:00，还剩 4 天 3 小时 · 跟随订阅的每周重置");

    const prev = resolveRange({ kind: "cycle", offset: -1 }, new Date("2026-10-15T10:00:00"), rule);
    expect(prev.timeRange).toEqual({ fromMs: anchorMs - 2 * WEEK_MS, toMs: anchorMs - WEEK_MS - 1 });
    expect(prev.detail).toBe("10月5日 周一 14:00 至 10月12日 周一 14:00 · 跟随订阅的每周重置");
  });

  it("月份：toMs 为下月初 - 1，支持跨年翻页且不带订阅说明", () => {
    const anchorRule: CycleRule = { kind: "anchor", anchorMs: at("2026-10-19T14:00:00") };
    const cur = resolveRange({ kind: "month", offset: 0 }, new Date("2026-10-15T10:00:00"), anchorRule);
    expect(cur.timeRange).toEqual({ fromMs: at("2026-10-01T00:00:00"), toMs: at("2026-11-01T00:00:00") - 1 });
    expect(cur.label).toBe("本月");
    expect(cur.detail).toMatch(/^2026 年 10 月，还剩 16 天 \d+ 小时$/);

    const back = resolveRange({ kind: "month", offset: -10 }, new Date("2026-10-15T10:00:00"), weeklyRule);
    expect(back.timeRange).toEqual({ fromMs: at("2025-12-01T00:00:00"), toMs: at("2026-01-01T00:00:00") - 1 });
    expect(back.label).toBe("2025 年 12 月");
  });

  it("最近 N 天：终点向下取整到 5 分钟；全部无上下界；两者均不可翻页", () => {
    const floored = at("2026-10-15T10:00:00");
    const r = resolveRange({ kind: "last30", offset: 0 }, now, weeklyRule);
    expect(r.timeRange).toEqual({ fromMs: floored - 30 * 86_400_000, toMs: floored });
    expect(r.label).toBe("最近 30 天");

    const r7 = resolveRange({ kind: "last7", offset: 0 }, new Date("2026-10-15T10:09:59.999"), weeklyRule);
    expect(r7.timeRange).toEqual({ fromMs: at("2026-10-08T10:05:00"), toMs: at("2026-10-15T10:05:00") });

    expect(resolveRange({ kind: "all", offset: 0 }, now, weeklyRule)).toMatchObject({
      timeRange: null,
      label: "全部",
    });
    expect(pageable("last7")).toBe(false);
    expect(pageable("cycle")).toBe(true);
    expect(pageable("day")).toBe(true);
  });

  it("天：标签、说明（今天带剩余时间）、上下界、跨月跨年 offset 与对照 / 前 7 天范围", () => {
    const today = resolveRange({ kind: "day", offset: 0 }, new Date("2026-10-15T10:15:00"), weeklyRule);
    expect(today.timeRange).toEqual({
      fromMs: at("2026-10-15T00:00:00"),
      toMs: at("2026-10-16T00:00:00") - 1,
    });
    expect(today.startMs).toBe(at("2026-10-15T00:00:00"));
    expect(today.endMs).toBe(at("2026-10-16T00:00:00"));
    expect(today.label).toBe("今天");
    expect(today.detail).toBe("10月15日 周四，还剩 13 小时 45 分");

    const yest = resolveRange({ kind: "day", offset: -1 }, new Date("2026-10-15T10:15:00"), weeklyRule);
    expect(yest.timeRange).toEqual({
      fromMs: at("2026-10-14T00:00:00"),
      toMs: at("2026-10-15T00:00:00") - 1,
    });
    expect(yest.label).toBe("昨天");
    expect(yest.detail).toBe("10月14日 周三");

    // Cross-month offset from Oct 2 back 3 days -> Sep 29.
    const crossMonth = resolveRange({ kind: "day", offset: -3 }, new Date("2026-10-02T08:00:00"), weeklyRule);
    expect(crossMonth.timeRange).toEqual({
      fromMs: at("2026-09-29T00:00:00"),
      toMs: at("2026-09-30T00:00:00") - 1,
    });
    expect(crossMonth.label).toBe("9月29日");
    expect(crossMonth.detail).toBe("9月29日 周二");

    // Cross-year offset from Jan 1 back 1 day -> Dec 31.
    const crossYear = resolveRange({ kind: "day", offset: -1 }, new Date("2027-01-01T08:00:00"), weeklyRule);
    expect(crossYear.timeRange).toEqual({
      fromMs: at("2026-12-31T00:00:00"),
      toMs: at("2027-01-01T00:00:00") - 1,
    });
    expect(crossYear.label).toBe("昨天");
    expect(crossYear.detail).toBe("12月31日 周四");

    // Calendar day offset helper.
    const refNow = new Date("2026-11-25T18:30:00");
    expect(dayOffset("2026-11-25", refNow)).toBe(0);
    expect(dayOffset("2026-11-24", refNow)).toBe(-1);
    expect(dayOffset("2026-10-25", refNow)).toBe(-31);

    // Previous 7 full days before Nov 24 -> Nov 17 00:00 .. Nov 24 00:00 - 1.
    const prev7 = resolvePrev7Range(-1, refNow);
    expect(prev7).toEqual({
      timeRange: { fromMs: at("2026-11-17T00:00:00"), toMs: at("2026-11-24T00:00:00") - 1 },
      startMs: at("2026-11-17T00:00:00"),
      endMs: at("2026-11-24T00:00:00"),
    });

    // Day compare ranges for Nov 24 (offset -1): prevDay -> Nov 23, prevWeek -> Nov 17.
    expect(resolveDayCompareRange(-1, "prevDay", refNow, weeklyRule)).toEqual({
      timeRange: { fromMs: at("2026-11-23T00:00:00"), toMs: at("2026-11-24T00:00:00") - 1 },
      startMs: at("2026-11-23T00:00:00"),
      endMs: at("2026-11-24T00:00:00"),
    });
    expect(resolveDayCompareRange(-1, "prevWeek", refNow, weeklyRule)).toEqual({
      timeRange: { fromMs: at("2026-11-17T00:00:00"), toMs: at("2026-11-18T00:00:00") - 1 },
      startMs: at("2026-11-17T00:00:00"),
      endMs: at("2026-11-18T00:00:00"),
    });
  });
});
