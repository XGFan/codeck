import type { HourUsage, Stats } from "@/ipc/bindings";
import type { PlanState } from "@/features/quota";
import { formatCost } from "@/lib/format";
import type { RangeSel } from "./range";

export type TrendPoint = [elapsedMs: number, cumulativeCost: number];

/**
 * Only uses the previous-period query's data when it succeeded without error:
 * after a background refetch failure TanStack Query keeps the stale `data` alongside `error`, which must be hidden.
 */
export function usablePrevStats(query: {
  data: Stats | undefined;
  status: "pending" | "error" | "success";
  error: unknown;
}): Stats | undefined {
  return query.status === "success" && query.error == null ? query.data : undefined;
}

/**
 * Builds a monotonically non-decreasing cumulative Cost curve `[elapsedMs, cost]` from `hourly` buckets.
 * For in-progress ranges, pass `cutoffMs = nowMs` so the curve stops at `nowMs`.
 */
export function buildCumulativeSeries(
  hourly: HourUsage[],
  rangeStartMs: number,
  rangeEndMs: number,
  cutoffMs?: number,
): TrendPoint[] {
  const totalMs = Math.max(1, rangeEndMs - rangeStartMs);
  const limitMs = cutoffMs != null ? Math.min(rangeEndMs, Math.max(rangeStartMs, cutoffMs)) : null;
  const sorted = [...hourly].sort((a, b) => a.startMs - b.startMs);

  const pts: TrendPoint[] = [[0, 0]];
  let cum = 0;

  for (const b of sorted) {
    if (limitMs != null && b.startMs >= limitMs) break;
    cum += Math.max(0, b.cost);
    if (limitMs != null && b.endMs > limitMs) {
      const x = Math.min(totalMs, Math.max(0, limitMs - rangeStartMs));
      pts.push([x, cum]);
      return pts;
    }
    const x = Math.min(totalMs, Math.max(0, b.endMs - rangeStartMs));
    const last = pts[pts.length - 1]!;
    if (x === last[0]) last[1] = cum;
    else pts.push([x, cum]);
  }

  const endX = limitMs != null ? Math.min(totalMs, Math.max(0, limitMs - rangeStartMs)) : totalMs;
  const last = pts[pts.length - 1]!;
  if (last[0] < endX) pts.push([endX, cum]);
  return pts;
}

/** Linearly interpolates a cumulative polyline at `elapsedMs` (approximate within-bucket comparison). */
export function valueAtElapsed(pts: TrendPoint[], elapsedMs: number): number {
  if (pts.length === 0) return 0;
  if (elapsedMs <= pts[0]![0]) return pts[0]![1];
  const last = pts[pts.length - 1]!;
  if (elapsedMs >= last[0]) return last[1];
  for (let i = 1; i < pts.length; i++) {
    const [x1, y1] = pts[i]!;
    if (elapsedMs <= x1) {
      const [x0, y0] = pts[i - 1]!;
      if (x1 === x0) return y1;
      return y0 + ((elapsedMs - x0) / (x1 - x0)) * (y1 - y0);
    }
  }
  return last[1];
}

export interface TrendLabels {
  current: string;
  previous: string;
}

export function trendLabels(sel: RangeSel): TrendLabels {
  if (sel.kind === "last7") return { current: "最近 7 天", previous: "前 7 天" };
  if (sel.offset === 0) return { current: "本周期", previous: "上一周期" };
  return { current: sel.offset === -1 ? "上一周期" : "该周期", previous: "前一周期" };
}

export interface TrendSummary {
  inProgress: boolean;
  nowElapsedMs: number | null;
  currentCost: number;
  diff: number | null;
  subtitle: string;
}

/** Computes headline text for 「用量走势」; omits diff when `prevPts` is absent or main query is placeholder. */
export function computeTrendSummary(args: {
  sel: RangeSel;
  rangeStartMs: number;
  rangeEndMs: number;
  nowMs: number;
  currentPts: TrendPoint[];
  prevPts: TrendPoint[] | null;
  isPlaceholderData: boolean;
}): TrendSummary {
  const { sel, rangeStartMs, rangeEndMs, nowMs, currentPts, prevPts, isPlaceholderData } = args;
  const inProgress = rangeEndMs > nowMs && rangeStartMs <= nowMs;
  const nowElapsedMs = inProgress ? Math.max(0, nowMs - rangeStartMs) : null;
  const currentCost = currentPts.length > 0 ? currentPts[currentPts.length - 1]![1] : 0;
  const labels = trendLabels(sel);

  if (!prevPts || prevPts.length === 0 || isPlaceholderData) {
    return {
      inProgress,
      nowElapsedMs,
      currentCost,
      diff: null,
      subtitle: inProgress ? `到现在 ${formatCost(currentCost)}` : `共 ${formatCost(currentCost)}`,
    };
  }

  const prevVal = inProgress ? valueAtElapsed(prevPts, nowElapsedMs!) : prevPts[prevPts.length - 1]![1];
  const diff = currentCost - prevVal;
  const dir = diff >= 0 ? "多" : "少";
  const subtitle = inProgress
    ? `到现在 ${formatCost(currentCost)} · 比${labels.previous}同一时刻${dir} ${formatCost(Math.abs(diff))}`
    : `共 ${formatCost(currentCost)} · 比${labels.previous}${dir} ${formatCost(Math.abs(diff))}`;

  return { inProgress, nowElapsedMs, currentCost, diff, subtitle };
}

export interface QuotaEstimate {
  estimatedCost: number;
  weeklyPercent: number;
  lineLabel: string;
  legendText: string;
}

/**
 * Estimated weekly Cost ceiling, shown ONLY when:
 * 1. range is 本周期 (`cycle`, `offset === 0`);
 * 2. main stats query is not placeholder data;
 * 3. plan usage state is `ok`;
 * 4. `weeklyAll` exists with `resetsMs > nowMs` and `percent >= 5`;
 * 5. `usedCost > 0`.
 */
export function computeQuotaEstimate(args: {
  sel: RangeSel;
  planState: PlanState;
  usedCost: number;
  nowMs: number;
  isPlaceholderData: boolean;
}): QuotaEstimate | null {
  const { sel, planState, usedCost, nowMs, isPlaceholderData } = args;
  if (sel.kind !== "cycle" || sel.offset !== 0) return null;
  if (isPlaceholderData) return null;
  if (planState.kind !== "ok") return null;
  if (!(usedCost > 0)) return null;

  const weekly = planState.usage.limits.find((l) => l.kind === "weeklyAll");
  if (!weekly || weekly.resetsMs == null || !Number.isFinite(weekly.resetsMs) || weekly.resetsMs <= nowMs) return null;
  if (!Number.isFinite(weekly.percent) || weekly.percent < 5) return null;

  const estimatedCost = usedCost / (weekly.percent / 100);
  const weeklyPercent = Math.round(weekly.percent);
  return {
    estimatedCost,
    weeklyPercent,
    lineLabel: `预估额度 ${formatCost(estimatedCost)}`,
    legendText: `预估额度 = 已用 Cost ÷ 本周已用 ${weeklyPercent}%`,
  };
}

/**
 * Clean y-axis of the trend chart: a ceiling above `peak` (max of current curve, reference curve, and estimated
 * quota) with headroom for the estimate's label, and the tick step. Of the round steps giving at most 5 intervals,
 * the one with the lowest ceiling (so the curves use the height), then the finer one.
 */
export function trendYAxis(peak: number): { max: number; step: number } {
  if (!(peak > 0) || !Number.isFinite(peak)) return { max: 1, step: 1 };
  const target = peak * 1.12;
  const mag = 10 ** Math.floor(Math.log10(target / 5));
  let best: { max: number; step: number } | null = null;
  for (const m of [1, 2, 2.5, 5, 10, 20, 25, 50]) {
    const step = m * mag;
    const n = Math.ceil(target / step - 1e-9);
    if (n > 5) continue;
    const max = n * step;
    if (!best || max < best.max * (1 - 1e-9)) best = { max, step };
  }
  return best ?? { max: target, step: target };
}

export const trendYAxisMax = (peak: number): number => trendYAxis(peak).max;
