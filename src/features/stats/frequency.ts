import type { DailyModelTokens, HourUsage, TimeRange } from "@/ipc/bindings";
import { formatCount } from "@/lib/format";
import { dayKey, parseDay } from "./charts";
import { resolveDaySpan } from "./usage";

export type FrequencyMode = "hourly" | "calendar" | "months";
export type HeatLevel = 0 | 1 | 2 | 3 | 4;

export interface HeatScale {
  q1: number;
  q2: number;
  q3: number;
  /** True only when every positive value is equal (`min === max`). */
  uniform: boolean;
}

export const MAX_MONTH_BLOCKS = 36;
export const CALENDAR_MAX_MS = 31 * 86_400_000 + 2 * 3_600_000;
export const WEEKDAYS_MON = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"] as const;
const WEEKDAYS_SUN = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"] as const;

/**
 * Picks which visualization 「使用频率」 uses:
 * - `hourly` when `hourlyCount > 0`;
 * - `calendar` when bounded `durationMs <= 31d + 2h` (tolerant of DST), or for unbounded ranges when `spanDays <= 31`;
 * - `months` otherwise.
 */
export function chooseFrequencyMode(hourlyCount: number, durationMs: number | null, spanDays: number): FrequencyMode {
  if (hourlyCount > 0) return "hourly";
  if (durationMs != null) return durationMs <= CALENDAR_MAX_MS ? "calendar" : "months";
  return spanDays <= 31 ? "calendar" : "months";
}

/** Q1 / Q2 / Q3 thresholds over strictly positive values. */
export function quartileThresholds(values: number[]): HeatScale {
  const nz = values.filter((v) => v > 0).sort((a, b) => a - b);
  if (nz.length === 0) return { q1: 0, q2: 0, q3: 0, uniform: false };
  const at = (p: number) => nz[Math.min(nz.length - 1, Math.floor((nz.length - 1) * p))]!;
  return { q1: at(0.25), q2: at(0.5), q3: at(0.75), uniform: nz[0] === nz[nz.length - 1] };
}

/** Maps a request count into 5 heat levels: 0 = zero, 1–4 = quartiles of non-zero counts. */
export function colorLevel(value: number, scale: HeatScale): HeatLevel {
  if (value <= 0) return 0;
  if (scale.uniform) return 4;
  if (value <= scale.q1) return 1;
  if (value <= scale.q2) return 2;
  if (value <= scale.q3) return 3;
  return 4;
}

export interface HourlyCell {
  row: number; // 0 = Mon .. 6 = Sun
  hour: number; // 0 .. 23
  requests: number;
  level: HeatLevel;
  future: boolean;
  isStart: boolean;
  title: string;
}

export interface HourlyGrid {
  /** 7 rows (Mon..Sun) × 24 hours. */
  rows: HourlyCell[][];
  /** Number of hourly buckets with requests > 0. */
  activeHours: number;
  /** Busiest folded cell, or null when all zero. */
  busiest: { row: number; hour: number; requests: number } | null;
  subtitle: string;
}

/**
 * Folds `hourly` buckets into a 7 (Mon..Sun) × 24 grid in local time:
 * - Multiple buckets falling on the same (weekday, hour) sum together;
 * - Cells with no bucket (DST gap) render as 0;
 * - Cells whose buckets all start strictly after `nowMs` render as empty future boxes;
 * - The cell containing `startMs` gets `isStart = true`.
 */
export function buildHourlyGrid(hourly: HourUsage[], startMs: number | null, nowMs: number): HourlyGrid {
  const cells = Array.from({ length: 7 }, (_, row) =>
    Array.from({ length: 24 }, (_, hour) => ({
      row,
      hour,
      requests: 0,
      hasPast: false,
      hasFuture: false,
    })),
  );

  let activeHours = 0;
  for (const b of hourly) {
    if (b.requests > 0) activeHours++;
    const d = new Date(b.startMs);
    const row = (d.getDay() + 6) % 7;
    const hour = d.getHours();
    const cell = cells[row]![hour]!;
    if (b.startMs > nowMs) {
      cell.hasFuture = true;
    } else {
      cell.hasPast = true;
      cell.requests += b.requests;
    }
  }

  const startAnchor = startMs ?? hourly[0]?.startMs ?? null;
  const startPos =
    startAnchor != null
      ? { row: (new Date(startAnchor).getDay() + 6) % 7, hour: new Date(startAnchor).getHours() }
      : null;

  const nonFutureVals: number[] = [];
  let busiest: { row: number; hour: number; requests: number } | null = null;
  for (const row of cells) {
    for (const c of row) {
      const future = !c.hasPast && c.hasFuture;
      if (!future) {
        nonFutureVals.push(c.requests);
        if (c.requests > 0 && (!busiest || c.requests > busiest.requests)) {
          busiest = { row: c.row, hour: c.hour, requests: c.requests };
        }
      }
    }
  }

  const thresholds = quartileThresholds(nonFutureVals);
  const rows: HourlyCell[][] = cells.map((row) =>
    row.map((c) => {
      const future = !c.hasPast && c.hasFuture;
      const isStart = startPos != null && startPos.row === c.row && startPos.hour === c.hour;
      const wd = WEEKDAYS_MON[c.row]!;
      return {
        row: c.row,
        hour: c.hour,
        requests: c.requests,
        level: future ? 0 : colorLevel(c.requests, thresholds),
        future,
        isStart,
        title: future ? "" : `${wd} ${c.hour} 时 · ${formatCount(c.requests)} 次请求`,
      };
    }),
  );

  const subtitle = busiest
    ? `${activeHours} 个小时有请求 · 最忙 ${WEEKDAYS_MON[busiest.row]} ${busiest.hour} 时`
    : "0 个小时有请求";

  return { rows, activeHours, busiest, subtitle };
}

export interface DayCell {
  /** `YYYY-MM-DD`. */
  day: string;
  dayOfMonth: number;
  /** Calendar display label: `M/D` on the first in-range cell and on the 1st of any month, `D` otherwise. */
  label: string;
  inRange: boolean;
  future: boolean;
  isToday: boolean;
  requests: number;
  level: HeatLevel;
  title: string;
}

export interface DaySummary {
  activeDays: number;
  busiest: { day: string; requests: number } | null;
  subtitle: string;
}

function dailyRequestMap(daily: DailyModelTokens[]): Map<string, number> {
  const m = new Map<string, number>();
  for (const d of daily) m.set(d.day, (m.get(d.day) ?? 0) + d.requests);
  return m;
}

function formatDayTitle(day: string, requests: number): string {
  const d = parseDay(day);
  return `${d.getMonth() + 1}月${d.getDate()}日 ${WEEKDAYS_SUN[d.getDay()]} · ${formatCount(requests)} 次请求`;
}

function summarizeDays(
  firstDay: string,
  lastDay: string,
  todayKey: string,
  reqMap: Map<string, number>,
): { summary: DaySummary; thresholds: HeatScale } {
  const vals: number[] = [];
  let activeDays = 0;
  let busiest: { day: string; requests: number } | null = null;
  const end = parseDay(lastDay);
  for (let d = parseDay(firstDay); d <= end; d.setDate(d.getDate() + 1)) {
    const key = dayKey(d);
    if (key > todayKey) continue;
    const reqs = reqMap.get(key) ?? 0;
    vals.push(reqs);
    if (reqs > 0) {
      activeDays++;
      if (!busiest || reqs > busiest.requests) busiest = { day: key, requests: reqs };
    }
  }
  let subtitle = `${activeDays} 天有请求`;
  if (busiest) {
    const bd = parseDay(busiest.day);
    subtitle += ` · 最忙 ${bd.getMonth() + 1}月${bd.getDate()}日 ${WEEKDAYS_SUN[bd.getDay()]}，${formatCount(busiest.requests)} 次`;
  }
  return { summary: { activeDays, busiest, subtitle }, thresholds: quartileThresholds(vals) };
}

export interface CalendarGrid {
  cells: DayCell[];
  summary: DaySummary;
}

/** Single-calendar view (Mon..Sun columns) for ranges `<= 31` days (months, last 30 days). */
export function buildCalendarGrid(
  daily: DailyModelTokens[],
  timeRange: TimeRange | null,
  nowMs: number,
): CalendarGrid | null {
  const span = resolveDaySpan(timeRange, daily, nowMs);
  if (!span) return null;
  const todayKey = dayKey(new Date(nowMs));
  const reqMap = dailyRequestMap(daily);
  const { summary, thresholds } = summarizeDays(span.firstDay, span.lastDay, todayKey, reqMap);

  const first = parseDay(span.firstDay);
  const lead = (first.getDay() + 6) % 7;
  const gridStart = new Date(first);
  gridStart.setDate(gridStart.getDate() - lead);

  const last = parseDay(span.lastDay);
  const trail = 6 - ((last.getDay() + 6) % 7);
  const gridEnd = new Date(last);
  gridEnd.setDate(gridEnd.getDate() + trail);

  const cells: DayCell[] = [];
  for (let d = new Date(gridStart); d <= gridEnd; d.setDate(d.getDate() + 1)) {
    const key = dayKey(d);
    const inRange = key >= span.firstDay && key <= span.lastDay;
    const future = inRange && key > todayKey;
    const requests = inRange && !future ? (reqMap.get(key) ?? 0) : 0;
    const dayOfMonth = d.getDate();
    const label =
      inRange && (key === span.firstDay || dayOfMonth === 1) ? `${d.getMonth() + 1}/${dayOfMonth}` : String(dayOfMonth);
    cells.push({
      day: key,
      dayOfMonth,
      label,
      inRange,
      future,
      isToday: inRange && key === todayKey,
      requests,
      level: inRange && !future ? colorLevel(requests, thresholds) : 0,
      title: inRange && !future ? formatDayTitle(key, requests) : "",
    });
  }
  return { cells, summary };
}

export interface MonthBlock {
  year: number;
  month: number; // 1..12
  label: string;
  /** Leading Monday-aligned transparent placeholders (`0..6`). */
  leadBlanks: number;
  days: DayCell[];
}

export interface MonthBlocksView {
  /** `2` when `blocks.length <= 4` (e.g. 最近 90 天), `4` when more (grouped by year, e.g. 全部). */
  columns: 2 | 4;
  groupedByYear: boolean;
  years: { year: number; blocks: MonthBlock[] }[];
  summary: DaySummary;
}

/** Mini month-block grid for ranges `> 31` days (最近 90 天, 全部), capped at the most recent 36 months. */
export function buildMonthBlocks(
  daily: DailyModelTokens[],
  timeRange: TimeRange | null,
  nowMs: number,
): MonthBlocksView | null {
  const span = resolveDaySpan(timeRange, daily, nowMs);
  if (!span) return null;
  const todayKey = dayKey(new Date(nowMs));
  const reqMap = dailyRequestMap(daily);

  const first = parseDay(span.firstDay);
  const last = parseDay(span.lastDay);
  const end = new Date(last.getFullYear(), last.getMonth(), 1);
  const earliestAllowed = new Date(end.getFullYear(), end.getMonth() - (MAX_MONTH_BLOCKS - 1), 1);
  const rawStart = new Date(first.getFullYear(), first.getMonth(), 1);
  const cur = rawStart < earliestAllowed ? earliestAllowed : rawStart;
  const effectiveFirstDay = dayKey(cur) > span.firstDay ? dayKey(cur) : span.firstDay;
  const { summary, thresholds } = summarizeDays(effectiveFirstDay, span.lastDay, todayKey, reqMap);

  const blocks: MonthBlock[] = [];
  for (; cur <= end; cur.setMonth(cur.getMonth() + 1)) {
    const year = cur.getFullYear();
    const month = cur.getMonth() + 1;
    const leadBlanks = (cur.getDay() + 6) % 7;
    const daysInMonth = new Date(year, month, 0).getDate();
    const days: DayCell[] = [];
    for (let dd = 1; dd <= daysInMonth; dd++) {
      const key = `${year}-${String(month).padStart(2, "0")}-${String(dd).padStart(2, "0")}`;
      const inRange = key >= effectiveFirstDay && key <= span.lastDay;
      const future = key > todayKey;
      const requests = inRange && !future ? (reqMap.get(key) ?? 0) : 0;
      days.push({
        day: key,
        dayOfMonth: dd,
        label: String(dd),
        inRange,
        future,
        isToday: inRange && key === todayKey,
        requests,
        level: inRange && !future ? colorLevel(requests, thresholds) : 0,
        title: inRange && !future ? formatDayTitle(key, requests) : "",
      });
    }
    blocks.push({ year, month, label: `${month}月`, leadBlanks, days });
  }

  const groupedByYear = blocks.length > 4;
  const columns: 2 | 4 = groupedByYear ? 4 : 2;
  const years: { year: number; blocks: MonthBlock[] }[] = [];
  for (const b of blocks) {
    const lastYr = years[years.length - 1];
    if (lastYr && lastYr.year === b.year) lastYr.blocks.push(b);
    else years.push({ year: b.year, blocks: [b] });
  }

  return { columns, groupedByYear, years, summary };
}
