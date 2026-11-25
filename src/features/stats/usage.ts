import type { DailyModelTokens, TimeRange } from "@/ipc/bindings";
import { dayKey, FAMILIES, family, parseDay, type ModelFamily } from "./charts";

export type UsageMetric = "cost" | "input" | "output";
export type UsageBucket = "1d" | "1w" | "1mo";

export const MAX_USAGE_BARS = 170;
const DAY_MS = 86_400_000;

/** Total input tokens across a row: uncached input + cache reads + cache writes. */
export const inputOf = (d: DailyModelTokens): number => d.input + d.cacheRead + d.cacheCreation;

/** Metric value of a single `DailyModelTokens` row. */
export function metricValue(d: DailyModelTokens, metric: UsageMetric): number {
  switch (metric) {
    case "cost":
      return d.cost;
    case "input":
      return inputOf(d);
    case "output":
      return d.output;
  }
}

/** Input tokens over the range: uncached input, cache reads, and cache writes. */
export function inputTotals(daily: DailyModelTokens[]): { input: number; read: number; write: number } {
  return daily.reduce(
    (a, d) => ({ input: a.input + d.input, read: a.read + d.cacheRead, write: a.write + d.cacheCreation }),
    { input: 0, read: 0, write: 0 },
  );
}

function utcDayMs(day: string): number {
  const [y, m, d] = day.split("-").map(Number);
  return Date.UTC(y!, m! - 1, d!);
}

/** Local Monday (`YYYY-MM-DD`) of the week containing `day`. */
export function mondayOf(day: string): string {
  const d = parseDay(day);
  const back = (d.getDay() + 6) % 7;
  d.setDate(d.getDate() - back);
  return dayKey(d);
}

/** Inclusive `[firstDay, lastDay]` calendar span for a range (`YYYY-MM-DD`), or null when `all` has no data. */
export function resolveDaySpan(
  timeRange: TimeRange | null,
  daily: DailyModelTokens[],
  nowMs: number,
): { firstDay: string; lastDay: string; spanDays: number } | null {
  let firstDay: string;
  let lastDay: string;
  if (timeRange?.fromMs != null && timeRange?.toMs != null) {
    firstDay = dayKey(new Date(timeRange.fromMs));
    lastDay = dayKey(new Date(timeRange.toMs));
  } else {
    if (daily.length === 0) return null;
    let minDay = daily[0]!.day;
    let maxDay = daily[0]!.day;
    for (const d of daily) {
      if (d.day < minDay) minDay = d.day;
      if (d.day > maxDay) maxDay = d.day;
    }
    const today = dayKey(new Date(nowMs));
    firstDay = minDay;
    lastDay = maxDay > today ? maxDay : today;
  }
  if (lastDay < firstDay) lastDay = firstDay;
  const spanDays = Math.round((utcDayMs(lastDay) - utcDayMs(firstDay)) / DAY_MS) + 1;
  return { firstDay, lastDay, spanDays };
}

/**
 * Elapsed days for 「日均」:
 * - Bounded range: `startMs` to `min(endMs, nowMs)`;
 * - `all`: local midnight of the earliest day in `daily` to `nowMs`;
 * - Under 1 day clamps to 1.
 */
export function elapsedDays(
  startMs: number | null,
  endMs: number | null,
  daily: DailyModelTokens[],
  nowMs: number,
): number {
  let ms: number;
  if (startMs != null && endMs != null) {
    ms = Math.min(endMs, nowMs) - startMs;
  } else {
    if (daily.length === 0) return 1;
    let minDay = daily[0]!.day;
    for (const d of daily) if (d.day < minDay) minDay = d.day;
    ms = nowMs - parseDay(minDay).getTime();
  }
  return Math.max(1, ms / DAY_MS);
}

/** Bar granularity: `<= 92d` daily (`1d`), `<= 2y` weekly (`1w`), `> 2y` monthly (`1mo`). */
export function chooseUsageBucket(spanDays: number): UsageBucket {
  if (spanDays <= 92) return "1d";
  if (spanDays <= 730) return "1w";
  return "1mo";
}

export interface UsageBucketItem {
  /** `YYYY-MM-DD` (day or Monday of week) or `YYYY-MM` (month). */
  key: string;
  /** Short x-axis label. */
  axisLabel: string;
  /** Tooltip title. */
  title: string;
  byFamily: Record<ModelFamily, number>;
}

export interface UsageSeries {
  bucket: UsageBucket;
  families: ModelFamily[];
  items: UsageBucketItem[];
}

const WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"] as const;

function emptyByFamily(): Record<ModelFamily, number> {
  return { fable: 0, opus: 0, sonnet: 0, haiku: 0, other: 0 };
}

/** Aggregates `daily` rows into contiguous `1d` / `1w` (Monday-aligned) / `1mo` buckets stacked by model family. */
export function buildUsageSeries(
  daily: DailyModelTokens[],
  metric: UsageMetric,
  timeRange: TimeRange | null,
  nowMs: number,
): UsageSeries {
  const span = resolveDaySpan(timeRange, daily, nowMs);
  if (!span) return { bucket: "1d", families: [], items: [] };

  const bucket = chooseUsageBucket(span.spanDays);
  const items: UsageBucketItem[] = [];
  const index = new Map<string, UsageBucketItem>();
  const shortWeekAxis = bucket === "1d" && span.spanDays <= 10;

  if (bucket === "1d") {
    const end = parseDay(span.lastDay);
    for (let d = parseDay(span.firstDay); d <= end; d.setDate(d.getDate() + 1)) {
      const key = dayKey(d);
      const isEdge = key === span.firstDay || key === span.lastDay;
      const axisLabel = shortWeekAxis
        ? isEdge
          ? `${WEEKDAYS[d.getDay()]} ${d.getDate()}`
          : WEEKDAYS[d.getDay()]!
        : `${d.getMonth() + 1}/${d.getDate()}`;
      const item: UsageBucketItem = {
        key,
        axisLabel,
        title: `${d.getMonth() + 1}月${d.getDate()}日 ${WEEKDAYS[d.getDay()]}`,
        byFamily: emptyByFamily(),
      };
      items.push(item);
      index.set(key, item);
    }
  } else if (bucket === "1w") {
    const firstMon = parseDay(mondayOf(span.firstDay));
    const lastMon = parseDay(mondayOf(span.lastDay));
    const multiYear = firstMon.getFullYear() !== lastMon.getFullYear();
    for (let d = new Date(firstMon); d <= lastMon; d.setDate(d.getDate() + 7)) {
      const key = dayKey(d);
      const end = new Date(d);
      end.setDate(end.getDate() + 6);
      const axisLabel =
        multiYear && d.getMonth() === 0 && d.getDate() <= 7
          ? `${d.getFullYear()}年1月`
          : `${d.getMonth() + 1}月${d.getDate()}日`;
      const item: UsageBucketItem = {
        key,
        axisLabel,
        title: `${d.getMonth() + 1}月${d.getDate()}日–${end.getMonth() + 1}月${end.getDate()}日`,
        byFamily: emptyByFamily(),
      };
      items.push(item);
      index.set(key, item);
    }
  } else {
    const first = parseDay(span.firstDay);
    const last = parseDay(span.lastDay);
    const end = new Date(last.getFullYear(), last.getMonth(), 1);
    const earliestAllowed = new Date(end.getFullYear(), end.getMonth() - (MAX_USAGE_BARS - 1), 1);
    const rawStart = new Date(first.getFullYear(), first.getMonth(), 1);
    const cur = rawStart < earliestAllowed ? earliestAllowed : rawStart;
    for (; cur <= end; cur.setMonth(cur.getMonth() + 1)) {
      const y = cur.getFullYear();
      const m = cur.getMonth() + 1;
      const key = `${y}-${String(m).padStart(2, "0")}`;
      const item: UsageBucketItem = {
        key,
        axisLabel: m === 1 ? `${y}年` : `${m}月`,
        title: `${y}年${m}月`,
        byFamily: emptyByFamily(),
      };
      items.push(item);
      index.set(key, item);
    }
  }

  for (const row of daily) {
    const k = bucket === "1d" ? row.day : bucket === "1w" ? mondayOf(row.day) : row.day.slice(0, 7);
    const target = index.get(k);
    if (!target) continue;
    target.byFamily[family(row.model)] += metricValue(row, metric);
  }

  const kept = items.length > MAX_USAGE_BARS ? items.slice(-MAX_USAGE_BARS) : items;
  const families = FAMILIES.filter((f) => kept.some((it) => it.byFamily[f] > 0));
  return { bucket, families, items: kept };
}

export interface ModelBreakdownRow {
  family: ModelFamily;
  value: number;
  /** Exact share in `[0, 100]` for bar widths. */
  shareExact: number;
  /** Rounded integer share for text display (`57%`). */
  sharePct: number;
  /** Every full model name of this family appearing in `daily` (passed to `Drill::Model`). */
  models: string[];
}

/** Metric breakdown by model family, sorted by metric value descending, with all full model names for drill-down. */
export function modelMetricBreakdown(daily: DailyModelTokens[], metric: UsageMetric): ModelBreakdownRow[] {
  const byFam = new Map<ModelFamily, { value: number; models: Set<string>; hasActivity: boolean }>();
  for (const d of daily) {
    const f = family(d.model);
    let entry = byFam.get(f);
    if (!entry) {
      entry = { value: 0, models: new Set(), hasActivity: false };
      byFam.set(f, entry);
    }
    const v = metricValue(d, metric);
    entry.value += v;
    if (v > 0 || d.requests > 0) {
      entry.models.add(d.model);
      entry.hasActivity = true;
    }
  }
  const total = [...byFam.values()].reduce((s, e) => s + e.value, 0);
  return FAMILIES.flatMap((f) => {
    const e = byFam.get(f);
    if (!e || !e.hasActivity || e.models.size === 0) return [];
    const shareExact = total > 0 ? (e.value / total) * 100 : 0;
    return [
      {
        family: f,
        value: e.value,
        shareExact,
        sharePct: Math.round(shareExact),
        models: [...e.models].sort(),
      },
    ];
  }).sort((a, b) => b.value - a.value || FAMILIES.indexOf(a.family) - FAMILIES.indexOf(b.family));
}
