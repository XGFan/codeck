import { describe, expect, it } from "vitest";
import { WEEK_MS, type CycleRule } from "@/lib/cycle";
import { pageable, resolveRange } from "./range";

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
  });
});
