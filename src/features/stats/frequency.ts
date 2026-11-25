import type { DailyModelTokens, HourUsage, TimeRange } from "@/ipc/bindings";
import { formatCount } from "@/lib/format";
import { dayKey, parseDay } from "./charts";
import { resolveDaySpan } from "./usage";

export type FrequencyMode = "hourly" | "months";
export type HeatLevel = 0 | 1 | 2 | 3 | 4;

export interface HeatScale {
  q1: number;
  q2: number;
  q3: number;
  /** True only when every positive value is equal (`min === max`). */
  uniform: boolean;
}

export const MAX_MONTH_BLOCKS = 36;
export const WEEKDAYS_MON = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"] as const;
const WEEKDAYS_SUN = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"] as const;

/** Picks which visualization 「使用频率」 uses: `hourly` when `hourlyCount > 0`, `months` otherwise. */
export function chooseFrequencyMode(hourlyCount: number): FrequencyMode {
  return hourlyCount > 0 ? "hourly" : "months";
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

export type HourlyCellKind = "out" | "data";

export interface HourlyCell {
  hour: number; // 0 .. 23
  kind: HourlyCellKind;
  requests: number;
  level: HeatLevel;
  /** Bold first line of the hover tooltip (`周二 10 时`), empty when not drawn. */
  tipTitle: string;
}

export interface HourlyRow {
  /** Local calendar day `YYYY-MM-DD`. */
  day: string;
  /** Local weekday label (`周一` .. `周日`). */
  weekday: string;
  /** 24 cells for hours `0..23`. */
  cells: HourlyCell[];
}

export interface HourlyGrid {
  /** One row per local calendar day touched by `[startMs, endMs)`, in chronological order. */
  rows: HourlyRow[];
  /** Number of hourly cells with requests > 0. */
  activeHours: number;
  /** Busiest cell (bucket), or null when all zero. */
  busiest: { day: string; weekday: string; hour: number; requests: number } | null;
  subtitle: string;
}

/**
 * Lays out `hourly` buckets chronologically, one row per local calendar day touched by `[startMs, endMs)`:
 * - Each row carries 24 hourly slots (`0..23`) and its weekday label (even if the entire row is in the future);
 * - Slots outside the range or whose bucket starts strictly after `nowMs` are transparent placeholders (`kind: "out"`);
 * - All other bucket slots render with their request count and quartile heat level (`kind: "data"`).
 */
export function buildHourlyGrid(
  hourly: HourUsage[],
  startMs: number | null,
  endMs: number | null,
  nowMs: number,
): HourlyGrid {
  const bucketMap = new Map<string, number>();
  for (const b of hourly) {
    if (b.startMs > nowMs) continue;
    const d = new Date(b.startMs);
    const key = `${dayKey(d)}:${d.getHours()}`;
    bucketMap.set(key, (bucketMap.get(key) ?? 0) + b.requests);
  }

  let firstDay: string | null = null;
  let lastDay: string | null = null;
  if (startMs != null && endMs != null && endMs > startMs) {
    firstDay = dayKey(new Date(startMs));
    lastDay = dayKey(new Date(endMs - 1));
  } else if (hourly.length > 0) {
    let minMs = hourly[0]!.startMs;
    let maxMs = hourly[0]!.startMs;
    for (const b of hourly) {
      if (b.startMs < minMs) minMs = b.startMs;
      if (b.startMs > maxMs) maxMs = b.startMs;
    }
    firstDay = dayKey(new Date(minMs));
    lastDay = dayKey(new Date(maxMs));
  }

  if (!firstDay || !lastDay) {
    return { rows: [], activeHours: 0, busiest: null, subtitle: "0 个小时有请求" };
  }

  interface RawSlot {
    hour: number;
    kind: HourlyCellKind;
    requests: number;
  }
  interface RawRow {
    day: string;
    weekday: string;
    slots: RawSlot[];
  }

  const rawRows: RawRow[] = [];
  const dataVals: number[] = [];
  let activeHours = 0;
  let busiest: { day: string; weekday: string; hour: number; requests: number } | null = null;

  const endDate = parseDay(lastDay);
  for (let d = parseDay(firstDay); d <= endDate; d.setDate(d.getDate() + 1)) {
    const day = dayKey(d);
    const weekday = WEEKDAYS_SUN[d.getDay()]!;
    const slots: RawSlot[] = [];
    for (let hour = 0; hour < 24; hour++) {
      const requests = bucketMap.get(`${day}:${hour}`);
      if (requests === undefined) {
        slots.push({ hour, kind: "out", requests: 0 });
      } else {
        slots.push({ hour, kind: "data", requests });
        dataVals.push(requests);
        if (requests > 0) {
          activeHours++;
          if (!busiest || requests > busiest.requests) {
            busiest = { day, weekday, hour, requests };
          }
        }
      }
    }
    rawRows.push({ day, weekday, slots });
  }

  const thresholds = quartileThresholds(dataVals);
  const rows: HourlyRow[] = rawRows.map(({ day, weekday, slots }) => ({
    day,
    weekday,
    cells: slots.map((s) => ({
      hour: s.hour,
      kind: s.kind,
      requests: s.requests,
      level: s.kind === "data" ? colorLevel(s.requests, thresholds) : 0,
      tipTitle: s.kind === "data" ? `${weekday} ${s.hour} 时` : "",
    })),
  }));

  const subtitle = busiest
    ? `${activeHours} 个小时有请求 · 最忙 ${busiest.weekday} ${busiest.hour} 时`
    : "0 个小时有请求";

  return { rows, activeHours, busiest, subtitle };
}

export interface DayCell {
  /** `YYYY-MM-DD`. */
  day: string;
  dayOfMonth: number;
  inRange: boolean;
  future: boolean;
  isToday: boolean;
  requests: number;
  level: HeatLevel;
  /** Bold first line of the hover tooltip (`10月14日 周三`), empty when not drawn. */
  tipTitle: string;
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

function formatDayTipTitle(day: string): string {
  const d = parseDay(day);
  return `${d.getMonth() + 1}月${d.getDate()}日 ${WEEKDAYS_SUN[d.getDay()]}`;
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

export interface MonthBlock {
  year: number;
  month: number; // 1..12
  label: string;
  /** Leading Monday-aligned transparent placeholders (`0..6`). */
  leadBlanks: number;
  days: DayCell[];
}

export interface MonthBlocksView {
  /** `1` for a single month (full-width with weekday header), `2` for 2–4 months, `4` when more (grouped by year). */
  columns: 1 | 2 | 4;
  groupedByYear: boolean;
  years: { year: number; blocks: MonthBlock[] }[];
  summary: DaySummary;
}

/** One block per calendar month touched by the range, capped at the most recent 36 months. */
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
      const drawn = inRange && !future;
      const requests = drawn ? (reqMap.get(key) ?? 0) : 0;
      days.push({
        day: key,
        dayOfMonth: dd,
        inRange,
        future,
        isToday: drawn && key === todayKey,
        requests,
        level: drawn ? colorLevel(requests, thresholds) : 0,
        tipTitle: drawn ? formatDayTipTitle(key) : "",
      });
    }
    blocks.push({ year, month, label: `${month}月`, leadBlanks, days });
  }

  const groupedByYear = blocks.length > 4;
  const columns: 1 | 2 | 4 = blocks.length === 1 ? 1 : groupedByYear ? 4 : 2;
  const years: { year: number; blocks: MonthBlock[] }[] = [];
  for (const b of blocks) {
    const lastYr = years[years.length - 1];
    if (lastYr && lastYr.year === b.year) lastYr.blocks.push(b);
    else years.push({ year: b.year, blocks: [b] });
  }

  return { columns, groupedByYear, years, summary };
}
