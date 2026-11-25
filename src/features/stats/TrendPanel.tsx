import { useMemo } from "react";
import type { Stats } from "@/ipc/bindings";
import type { PlanState } from "@/features/quota";
import { Chart } from "./Chart";
import { trendOption, type Theme } from "./charts";
import type { RangeSel } from "./range";
import { buildCumulativeSeries, computeQuotaEstimate, computeTrendSummary, trendLabels, trendYAxisMax } from "./trend";

/** Right column (bottom), shown only for cycles and last 7 days: cumulative Cost curve vs previous period + estimate. */
export function TrendPanel({
  sel,
  stats,
  prevStats,
  rangeStartMs,
  rangeEndMs,
  prevStartMs,
  prevEndMs,
  now,
  planState,
  isPlaceholderData,
  theme,
}: {
  sel: RangeSel;
  stats: Stats;
  prevStats: Stats | undefined;
  rangeStartMs: number;
  rangeEndMs: number;
  prevStartMs: number | null;
  prevEndMs: number | null;
  now: Date;
  planState: PlanState;
  isPlaceholderData: boolean;
  theme: Theme;
}) {
  const nowMs = now.getTime();
  const inProgress = rangeEndMs > nowMs && rangeStartMs <= nowMs;
  const labels = trendLabels(sel);

  const currentPts = useMemo(
    () => buildCumulativeSeries(stats.hourly, rangeStartMs, rangeEndMs, inProgress ? nowMs : undefined),
    [stats.hourly, rangeStartMs, rangeEndMs, inProgress, nowMs],
  );

  const prevPts = useMemo(() => {
    if (!prevStats || prevStartMs == null || prevEndMs == null) return null;
    if (prevStats.hourly.length === 0) return null;
    return buildCumulativeSeries(prevStats.hourly, prevStartMs, prevEndMs);
  }, [prevStats, prevStartMs, prevEndMs]);

  const summary = useMemo(
    () =>
      computeTrendSummary({
        sel,
        rangeStartMs,
        rangeEndMs,
        nowMs,
        currentPts,
        prevPts,
        isPlaceholderData,
      }),
    [sel, rangeStartMs, rangeEndMs, nowMs, currentPts, prevPts, isPlaceholderData],
  );

  const estimate = useMemo(
    () =>
      computeQuotaEstimate({
        sel,
        planState,
        usedCost: summary.currentCost,
        nowMs,
        isPlaceholderData,
      }),
    [sel, planState, summary.currentCost, nowMs, isPlaceholderData],
  );

  const option = useMemo(
    () =>
      trendOption({
        rangeStartMs,
        rangeEndMs,
        currentPts,
        prevPts,
        labels,
        nowElapsedMs: summary.nowElapsedMs,
        estimate,
        theme,
      }),
    [rangeStartMs, rangeEndMs, currentPts, prevPts, labels, summary.nowElapsedMs, estimate, theme],
  );

  const yMax = useMemo(() => {
    const curMax = currentPts.length ? currentPts[currentPts.length - 1]![1] : 0;
    const prevMax = prevPts?.length ? prevPts[prevPts.length - 1]![1] : 0;
    return trendYAxisMax(Math.max(curMax, prevMax, estimate?.estimatedCost ?? 0));
  }, [currentPts, prevPts, estimate]);

  return (
    <section
      data-testid="stats-trend"
      data-ymax={yMax}
      data-estimate={estimate ? estimate.estimatedCost.toFixed(2) : undefined}
      aria-label="用量走势"
      className="flex min-h-0 min-w-0 flex-col gap-2 rounded-[10px] border border-border bg-ground px-3.5 py-3"
    >
      <div className="flex items-center gap-3">
        <h2 className="m-0 flex-1 text-[13px] font-semibold">用量走势</h2>
        <span data-testid="trend-subtitle" className="truncate text-[11px] text-secondary tabular-nums">
          {summary.subtitle}
        </span>
      </div>

      <Chart
        testId="trend-chart"
        height={156}
        label={`从${labels.current}开始累计的 Cost${prevPts ? `，对照${labels.previous}` : ""}${estimate ? "与预估额度" : ""}`}
        option={option}
      />

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-secondary">
        <span className="inline-flex items-center gap-1">
          <i className="block h-[2px] w-[14px] rounded-[1px] bg-accent" aria-hidden />
          {labels.current}
        </span>
        {prevPts && (
          <span data-testid="trend-prev-legend" className="inline-flex items-center gap-1">
            <i className="block h-[2px] w-[14px] rounded-[1px]" style={{ background: theme.prevLine }} aria-hidden />
            {labels.previous}
          </span>
        )}
        {estimate && (
          <span data-testid="trend-estimate-legend" className="inline-flex items-center gap-1">
            <i className="block h-0 w-[14px] border-t-2 border-dashed border-live-waiting" aria-hidden />
            {estimate.legendText}
          </span>
        )}
        <span className="flex-1" />
        <span>累计 Cost</span>
      </div>
    </section>
  );
}
