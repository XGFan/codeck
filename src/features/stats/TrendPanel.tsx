import { useMemo } from "react";
import type { Stats } from "@/ipc/bindings";
import type { PlanState } from "@/features/quota";
import { cn } from "@/lib/cn";
import { Chart } from "./Chart";
import { trendOption, type Theme } from "./charts";
import type { DayCompare, RangeSel } from "./range";
import { buildCumulativeSeries, computeQuotaEstimate, computeTrendSummary, trendLabels, trendYAxisMax } from "./trend";

const DAY_COMPARE_OPTIONS: { key: DayCompare; label: string }[] = [
  { key: "prevDay", label: "前一天" },
  { key: "prevWeek", label: "上周期同一天" },
];

/** Right column (bottom), shown for days, cycles, and last 7 days: cumulative Cost curve vs reference + optional estimate. */
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
  dayCompare = "prevDay",
  onDayCompareChange,
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
  dayCompare?: DayCompare;
  onDayCompareChange?: (c: DayCompare) => void;
}) {
  const nowMs = now.getTime();
  const isDay = sel.kind === "day";
  const inProgress = rangeEndMs > nowMs && rangeStartMs <= nowMs;
  const labels = trendLabels(sel, dayCompare, prevStartMs);

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
        dayCompare,
      }),
    [sel, rangeStartMs, rangeEndMs, nowMs, currentPts, prevPts, isPlaceholderData, dayCompare],
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
        mode: isDay ? "day" : "week",
      }),
    [rangeStartMs, rangeEndMs, currentPts, prevPts, labels, summary.nowElapsedMs, estimate, theme, isDay],
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
        <h2 className={cn("m-0 text-[13px] font-semibold", isDay ? "shrink-0" : "flex-1")}>用量走势</h2>
        <span
          data-testid="trend-subtitle"
          className={cn("truncate text-[11px] text-secondary tabular-nums", isDay && "min-w-0 flex-1")}
        >
          {summary.subtitle}
        </span>
        {isDay && onDayCompareChange && (
          <div
            role="group"
            aria-label="对照"
            data-testid="day-compare"
            className="flex shrink-0 overflow-hidden rounded-[6px] border border-border bg-ground"
          >
            {DAY_COMPARE_OPTIONS.map((opt, idx) => {
              const active = dayCompare === opt.key;
              return (
                <button
                  key={opt.key}
                  type="button"
                  aria-pressed={active}
                  onClick={() => onDayCompareChange(opt.key)}
                  className={cn(
                    "h-6 px-2 text-[11px] outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent",
                    idx > 0 && "border-l border-border",
                    active ? "bg-accent/15 text-accent" : "text-secondary hover:bg-text/5",
                  )}
                >
                  {opt.label}
                </button>
              );
            })}
          </div>
        )}
      </div>

      <Chart
        testId="trend-chart"
        height={isDay ? 140 : 156}
        label={
          isDay
            ? `这一天累计的 Cost${prevPts ? `，对照${labels.previous}` : ""}`
            : `从${labels.current}开始累计的 Cost${prevPts ? `，对照${labels.previous}` : ""}${estimate ? "与预估额度" : ""}`
        }
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
            {labels.previousLegend ?? labels.previous}
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
