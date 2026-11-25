import { describe, expect, it } from "vitest";
import type { DailyModelTokens, HourUsage } from "@/ipc/bindings";
import {
  buildCalendarGrid,
  buildHourlyGrid,
  buildMonthBlocks,
  CALENDAR_MAX_MS,
  chooseFrequencyMode,
  colorLevel,
  quartileThresholds,
} from "./frequency";

const HOUR_MS = 3_600_000;
const DAY_MS = 86_400_000;

const dayRow = (day: string, requests: number): DailyModelTokens => ({
  day,
  model: "claude-opus-4-7",
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheCreation: 0,
  cost: 0,
  requests,
});

describe("quartileThresholds & colorLevel", () => {
  it("0 为第 0 档，其余按非零值四分位分 1–4 档", () => {
    const th = quartileThresholds([0, 0, 10, 20, 30, 40, 50, 60, 70, 80]);
    expect(colorLevel(0, th)).toBe(0);
    expect(colorLevel(10, th)).toBe(1);
    expect(colorLevel(20, th)).toBe(1);
    expect(colorLevel(40, th)).toBe(2);
    expect(colorLevel(60, th)).toBe(3);
    expect(colorLevel(80, th)).toBe(4);
  });

  it("所有非零值完全相同时归入最深档；仅 q1===q3 但不全相等时照常分档", () => {
    expect(colorLevel(0, quartileThresholds([0, 0]))).toBe(0);
    expect(colorLevel(5, quartileThresholds([5, 5]))).toBe(4);

    // [1, 1, 1, 1, 1, 100]: q1 = q2 = q3 = 1, but min !== max -> 1 gets level 1, 100 gets level 4.
    const skewed = quartileThresholds([1, 1, 1, 1, 1, 100]);
    expect(colorLevel(1, skewed)).toBe(1);
    expect(colorLevel(100, skewed)).toBe(4);
  });
});

describe("chooseFrequencyMode", () => {
  it("hourly 非空用小时格；有界按实际时长 ≤ 31 天 + 2 小时用月历（跨 32 个本机日期但时长 30 天仍是月历）；全部按日期跨度", () => {
    expect(chooseFrequencyMode(168, 7 * DAY_MS, 8)).toBe("hourly");
    // 30-day duration spanning 32 local calendar days (e.g. DST shift) still renders as a single calendar.
    expect(chooseFrequencyMode(0, 30 * DAY_MS, 32)).toBe("calendar");
    expect(chooseFrequencyMode(0, CALENDAR_MAX_MS, 32)).toBe("calendar");
    expect(chooseFrequencyMode(0, CALENDAR_MAX_MS + 1, 32)).toBe("months");
    // Unbounded (全部): uses calendar day span.
    expect(chooseFrequencyMode(0, null, 31)).toBe("calendar");
    expect(chooseFrequencyMode(0, null, 32)).toBe("months");
  });
});

describe("buildHourlyGrid", () => {
  it("折进 7×24 格，同格相加，未来画空框，无桶格按 0 画，标出起点与最忙格", () => {
    // 2026-11-23 is Monday.
    const startMs = new Date("2026-11-23T14:00:00").getTime();
    const nowMs = new Date("2026-11-25T14:30:00").getTime(); // Wednesday 14:30
    const hourly: HourUsage[] = [
      { startMs, endMs: startMs + HOUR_MS, requests: 10, cost: 1 }, // Mon 14h (past)
      {
        startMs: new Date("2026-11-24T02:00:00").getTime(),
        endMs: new Date("2026-11-24T03:00:00").getTime(),
        requests: 50,
        cost: 5,
      }, // Tue 2h
      // Second Mon 14h (if a range wraps same hour) would add, here add a past bucket on Tue 2h to verify sum:
      {
        startMs: new Date("2026-11-24T02:00:00").getTime(),
        endMs: new Date("2026-11-24T03:00:00").getTime(),
        requests: 25,
        cost: 2,
      },
      {
        startMs: new Date("2026-11-26T10:00:00").getTime(),
        endMs: new Date("2026-11-26T11:00:00").getTime(),
        requests: 0,
        cost: 0,
      }, // Thu 10h (future)
    ];
    const grid = buildHourlyGrid(hourly, startMs, nowMs);
    expect(grid.rows).toHaveLength(7);
    expect(grid.rows[0]!).toHaveLength(24);
    expect(grid.rows[0]![14]!.isStart).toBe(true);
    expect(grid.rows[0]![14]!.requests).toBe(10);
    expect(grid.rows[1]![2]!.requests).toBe(75); // 50 + 25 summed
    expect(grid.rows[3]![10]!.future).toBe(true); // Thu 10h is future
    expect(grid.rows[4]![0]!.future).toBe(false); // No bucket -> treated as 0, not future
    expect(grid.rows[4]![0]!.requests).toBe(0);
    expect(grid.activeHours).toBe(3); // counted by buckets with requests > 0
    expect(grid.busiest).toEqual({ row: 1, hour: 2, requests: 75 });
    expect(grid.subtitle).toBe("3 个小时有请求 · 最忙 周二 2 时");
  });
});

describe("buildCalendarGrid & buildMonthBlocks", () => {
  it("月历：范围外透明、未来画空框、今天描边、标题写有请求天数与最忙日", () => {
    const fromMs = new Date("2026-10-01T00:00:00").getTime();
    const toMs = new Date("2026-11-01T00:00:00").getTime() - 1;
    const nowMs = new Date("2026-10-20T12:00:00").getTime();
    const daily = [dayRow("2026-10-01", 200), dayRow("2026-10-14", 1840), dayRow("2026-10-20", 400)];
    const cal = buildCalendarGrid(daily, { fromMs, toMs }, nowMs)!;
    // Oct 1 2026 is Thursday -> 3 leading blanks (Mon..Wed).
    expect(cal.cells[0]!.inRange).toBe(false);
    expect(cal.cells[2]!.inRange).toBe(false);
    expect(cal.cells[3]!).toMatchObject({ day: "2026-10-01", label: "10/1", inRange: true, requests: 200 });
    expect(cal.cells[4]!).toMatchObject({ day: "2026-10-02", label: "2" });
    const today = cal.cells.find((c) => c.day === "2026-10-20")!;
    expect(today.isToday).toBe(true);
    expect(today.future).toBe(false);
    const future = cal.cells.find((c) => c.day === "2026-10-25")!;
    expect(future.future).toBe(true);
    expect(cal.summary.subtitle).toBe("3 天有请求 · 最忙 10月14日 周三，1,840 次");

    // 跨月月历（最近 30 天）：第一格与每月 1 号写 M/D，其余写日。
    const cross = buildCalendarGrid(
      [dayRow("2026-10-26", 10), dayRow("2026-11-01", 20)],
      {
        fromMs: new Date("2026-10-26T14:00:00").getTime(),
        toMs: new Date("2026-11-25T14:00:00").getTime(),
      },
      new Date("2026-11-25T14:00:00").getTime(),
    )!;
    expect(cross.cells.find((c) => c.day === "2026-10-26")!.label).toBe("10/26");
    expect(cross.cells.find((c) => c.day === "2026-10-27")!.label).toBe("27");
    expect(cross.cells.find((c) => c.day === "2026-11-01")!.label).toBe("11/1");
    expect(cross.cells.find((c) => c.day === "2026-11-02")!.label).toBe("2");
  });

  it("月块：≤4 块两列不按年分组，>4 块四列按年分组", () => {
    const nowMs = new Date("2026-11-25T12:00:00").getTime();
    const ninety = buildMonthBlocks(
      [dayRow("2026-09-10", 500), dayRow("2026-11-25", 100)],
      { fromMs: new Date("2026-08-28T00:00:00").getTime(), toMs: nowMs },
      nowMs,
    )!;
    expect(ninety.columns).toBe(2);
    expect(ninety.groupedByYear).toBe(false);
    expect(ninety.years[0]!.blocks.map((b) => b.label)).toEqual(["8月", "9月", "10月", "11月"]);

    const all = buildMonthBlocks([dayRow("2025-11-01", 100), dayRow("2026-11-25", 300)], null, nowMs)!;
    expect(all.columns).toBe(4);
    expect(all.groupedByYear).toBe(true);
    expect(all.years.map((y) => y.year)).toEqual([2025, 2026]);

    // 存在 1970 年异常记录时，月块最多画最近 36 个月，且标题小结不计入范围外的旧记录。
    const capped = buildMonthBlocks([dayRow("1970-01-01", 99999), dayRow("2026-11-25", 300)], null, nowMs)!;
    const totalBlocks = capped.years.reduce((s, y) => s + y.blocks.length, 0);
    expect(totalBlocks).toBe(36);
    expect(capped.years[0]!.year).toBe(2023);
    expect(capped.years[0]!.blocks[0]!.month).toBe(12);
    expect(capped.summary.activeDays).toBe(1);
    expect(capped.summary.busiest).toEqual({ day: "2026-11-25", requests: 300 });
  });
});
