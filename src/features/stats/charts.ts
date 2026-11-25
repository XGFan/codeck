import type { ChartOption } from "@/lib/echarts";
import { formatCost, formatTokens } from "@/lib/format";
import { trendYAxis, type QuotaEstimate, type TrendLabels, type TrendPoint } from "./trend";
import type { UsageMetric, UsageSeries } from "./usage";

/** Reference dataviz palette (validated): categorical slots 1-3 + 7, plus a raspberry/magenta for fable. */
export interface Theme {
  surface: string;
  ink: string;
  muted: string;
  grid: string;
  axis: string;
  tooltipBg: string;
  accent: string;
  prevLine: string;
  estimate: string;
  estimateText: string;
  futureArea: string;
  series: Record<ModelFamily, string>;
}
export type ModelFamily = "fable" | "opus" | "sonnet" | "haiku" | "other";
export const FAMILIES: ModelFamily[] = ["fable", "opus", "sonnet", "haiku", "other"];

export const themes: Record<"light" | "dark", Theme> = {
  light: {
    surface: "#ffffff",
    ink: "#52514e",
    muted: "#5f6064",
    grid: "#ececea",
    axis: "#d0d0cc",
    tooltipBg: "#ffffff",
    accent: "#0a5fcc",
    prevLine: "#b9b9b6",
    estimate: "#c27c0e",
    estimateText: "#8a5a0a",
    futureArea: "rgba(250,250,248,0.85)",
    series: { fable: "#b01870", opus: "#2a78d6", sonnet: "#eb6834", haiku: "#1baf7a", other: "#4a3aa7" },
  },
  dark: {
    surface: "#1e1e20",
    ink: "#ececee",
    muted: "#a1a1a6",
    grid: "#2c2c2f",
    axis: "#454549",
    tooltipBg: "#2c2c2f",
    accent: "#4c8df6",
    prevLine: "#6e6e73",
    estimate: "#e0a030",
    estimateText: "#f2c27c",
    futureArea: "rgba(35,35,37,0.65)",
    series: { fable: "#c020a0", opus: "#3987e5", sonnet: "#d95926", haiku: "#199e70", other: "#9085e9" },
  },
};

export const FAMILY_LABEL: Record<ModelFamily, string> = {
  fable: "fable",
  opus: "opus",
  sonnet: "sonnet",
  haiku: "haiku",
  other: "其他",
};

export function family(model: string): ModelFamily {
  const m = model.toLowerCase();
  return FAMILIES.find((f) => f !== "other" && m.includes(f)) ?? "other";
}

export function parseDay(day: string): Date {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y!, m! - 1, d!);
}

export function dayKey(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

const base = (t: Theme) => ({
  animation: false,
  textStyle: { color: t.ink, fontFamily: "inherit", fontSize: 10 },
  tooltip: {
    confine: true,
    backgroundColor: t.tooltipBg,
    borderColor: t.grid,
    textStyle: { color: t.ink, fontSize: 12 },
    extraCssText: "box-shadow:0 4px 14px rgba(0,0,0,.18);",
  },
});

/** Stacked bar chart for 「用量」 (Cost / input / output stacked by model family; 1d / 1w / 1mo). */
export function usageBarOption(usage: UsageSeries, metric: UsageMetric, t: Theme): ChartOption {
  const { bucket, families, items } = usage;
  const formatValue = metric === "cost" ? formatCost : formatTokens;
  const drillable = bucket === "1d";
  const series = families.map((f, i) => ({
    name: FAMILY_LABEL[f],
    type: "bar",
    stack: "usage",
    barMaxWidth: bucket === "1d" && items.length <= 10 ? 28 : 16,
    cursor: drillable ? "pointer" : "default",
    itemStyle: {
      color: t.series[f],
      borderRadius: i === families.length - 1 ? [2, 2, 0, 0] : 0,
    },
    emphasis: { focus: "series" },
    data: items.map((it) => it.byFamily[f]),
  }));

  return {
    ...base(t),
    grid: { left: 42, right: 8, top: 8, bottom: 22 },
    tooltip: {
      ...base(t).tooltip,
      trigger: "axis",
      axisPointer: { type: "shadow", shadowStyle: { color: t.grid, opacity: 0.45 } },
      formatter: (ps: { dataIndex: number; marker: string; seriesName: string; value: number }[]) => {
        const item = items[ps[0]!.dataIndex];
        if (!item) return "";
        const rows = ps
          .filter((p) => p.value > 0)
          .reverse()
          .map((p) => `${p.marker}${p.seriesName}　${formatValue(p.value)}`);
        return rows.length ? `<b>${item.title}</b><br/>${rows.join("<br/>")}` : `<b>${item.title}</b><br/>0`;
      },
    },
    xAxis: {
      type: "category",
      data: items.map((it) => it.key),
      axisTick: { show: false },
      axisLine: { lineStyle: { color: t.axis } },
      axisLabel: {
        color: t.muted,
        hideOverlap: true,
        formatter: (_val: string, idx: number) => items[idx]?.axisLabel ?? "",
      },
    },
    yAxis: {
      type: "value",
      splitNumber: 3,
      axisLabel: {
        color: t.muted,
        formatter: (v: number) => (v === 0 ? "0" : metric === "cost" ? `$${v}` : formatTokens(v)),
      },
      splitLine: { lineStyle: { color: t.grid } },
    },
    series,
  };
}

const WEEKDAYS_SUN = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"] as const;
const DAY_MS = 86_400_000;

/** Cumulative Cost line chart for 「用量走势」 (current curve, optional previous curve, "现在" line, optional estimate). */
export function trendOption(args: {
  rangeStartMs: number;
  rangeEndMs: number;
  currentPts: TrendPoint[];
  prevPts: TrendPoint[] | null;
  labels: TrendLabels;
  nowElapsedMs: number | null;
  estimate: QuotaEstimate | null;
  theme: Theme;
}): ChartOption {
  const { rangeStartMs, rangeEndMs, currentPts, prevPts, labels, nowElapsedMs, estimate, theme: t } = args;
  const totalMs = Math.max(1, rangeEndMs - rangeStartMs);

  const curMax = currentPts.length ? currentPts[currentPts.length - 1]![1] : 0;
  const prevMax = prevPts?.length ? prevPts[prevPts.length - 1]![1] : 0;
  const { max: yMax, step: yStep } = trendYAxis(Math.max(curMax, prevMax, estimate?.estimatedCost ?? 0));

  const markLines: unknown[] = [];
  if (nowElapsedMs != null) {
    // ECharts rotates `inside*` markLine labels along vertical lines; `position: "end"` keeps it horizontal at the top.
    const nearRight = nowElapsedMs / totalMs > 0.9;
    markLines.push({
      xAxis: nowElapsedMs,
      lineStyle: { color: t.ink, type: [2, 3], width: 1 },
      label: {
        show: true,
        formatter: "现在",
        position: "end",
        distance: 0,
        align: nearRight ? "right" : "left",
        verticalAlign: "top",
        padding: nearRight ? [2, 5, 0, 0] : [2, 0, 0, 5],
        color: t.ink,
        fontSize: 10,
      },
    });
  }
  if (estimate) {
    markLines.push({
      yAxis: estimate.estimatedCost,
      lineStyle: { color: t.estimate, type: [5, 3], width: 1.2 },
      label: {
        show: true,
        formatter: estimate.lineLabel,
        position: "insideStartTop",
        distance: [4, 4],
        color: t.estimateText,
        fontSize: 10,
      },
    });
  }

  const markArea =
    nowElapsedMs != null && nowElapsedMs < totalMs
      ? {
          silent: true,
          itemStyle: { color: t.futureArea },
          data: [[{ xAxis: nowElapsedMs }, { xAxis: totalMs }]],
        }
      : undefined;

  const series: unknown[] = [];
  if (prevPts && prevPts.length > 0) {
    series.push({
      name: labels.previous,
      type: "line",
      showSymbol: false,
      smooth: false,
      lineStyle: { color: t.prevLine, width: 1.3 },
      itemStyle: { color: t.prevLine },
      data: prevPts,
    });
  }
  series.push({
    name: labels.current,
    type: "line",
    showSymbol: false,
    smooth: false,
    lineStyle: { color: t.accent, width: 1.8 },
    itemStyle: { color: t.accent },
    areaStyle: { color: t.accent, opacity: 0.12 },
    data: currentPts,
    markLine: markLines.length ? { silent: true, symbol: ["none", "none"], data: markLines } : undefined,
    markArea,
  });

  return {
    ...base(t),
    grid: { left: 42, right: 12, top: 14, bottom: 22 },
    tooltip: {
      ...base(t).tooltip,
      trigger: "axis",
      formatter: (ps: { value: [number, number]; marker: string; seriesName: string }[]) => {
        if (!ps.length) return "";
        const elapsed = ps[0]!.value[0];
        const d = new Date(rangeStartMs + elapsed);
        const title = `${d.getMonth() + 1}月${d.getDate()}日 ${WEEKDAYS_SUN[d.getDay()]} ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
        const lines = ps.map((p) => `${p.marker}${p.seriesName}　${formatCost(p.value[1])}`);
        return `<b>${title}</b><br/>${lines.join("<br/>")}`;
      },
    },
    xAxis: {
      type: "value",
      min: 0,
      max: totalMs,
      interval: DAY_MS,
      axisTick: { show: false },
      axisLine: { lineStyle: { color: t.axis } },
      splitLine: { show: true, lineStyle: { color: t.grid } },
      axisLabel: {
        color: t.muted,
        hideOverlap: true,
        formatter: (v: number) => WEEKDAYS_SUN[new Date(rangeStartMs + v).getDay()]!,
      },
    },
    yAxis: {
      type: "value",
      min: 0,
      max: yMax,
      interval: yStep,
      axisLabel: {
        color: t.muted,
        formatter: (v: number) => (v === 0 ? "$0" : `$${v}`),
      },
      splitLine: { lineStyle: { color: t.grid } },
    },
    series,
  };
}
