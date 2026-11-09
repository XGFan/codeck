import type { Activity, ActivityBucket, DailyModelTokens } from "@/ipc/bindings";
import type { ChartOption } from "@/lib/echarts";
import { formatCost, formatTokens } from "@/lib/format";

/** Reference dataviz palette (validated): categorical slots 1-3 + 7. */
export interface Theme {
  surface: string;
  ink: string;
  muted: string;
  grid: string;
  axis: string;
  tooltipBg: string;
  series: Record<ModelFamily, string>;
  ioInput: string;
  ioOutput: string;
}
export type ModelFamily = "opus" | "sonnet" | "haiku" | "other";
export const FAMILIES: ModelFamily[] = ["opus", "sonnet", "haiku", "other"];

export const themes: Record<"light" | "dark", Theme> = {
  light: {
    surface: "#ffffff",
    ink: "#52514e",
    muted: "#898781",
    grid: "#e1e0d9",
    axis: "#c3c2b7",
    tooltipBg: "#ffffff",
    series: { opus: "#2a78d6", sonnet: "#eb6834", haiku: "#1baf7a", other: "#4a3aa7" },
    ioInput: "#86b6ef",
    ioOutput: "#1c5cab",
  },
  dark: {
    surface: "#1e1e20",
    ink: "#c3c2b7",
    muted: "#898781",
    grid: "#2c2c2a",
    axis: "#383835",
    tooltipBg: "#2c2c2f",
    series: { opus: "#3987e5", sonnet: "#d95926", haiku: "#199e70", other: "#9085e9" },
    ioInput: "#256abf",
    ioOutput: "#b7d3f6",
  },
};

export const FAMILY_LABEL: Record<ModelFamily, string> = {
  opus: "opus",
  sonnet: "sonnet",
  haiku: "haiku",
  other: "其他",
};

export function family(model: string): ModelFamily {
  const m = model.toLowerCase();
  return m.includes("opus") ? "opus" : m.includes("sonnet") ? "sonnet" : m.includes("haiku") ? "haiku" : "other";
}

export type Metric = "all" | "input" | "output" | "cost";

/** Input counts everything the model read: new input plus cache reads and writes. */
const inputOf = (d: DailyModelTokens) => d.input + d.cacheRead + d.cacheCreation;
const metricValue = (d: DailyModelTokens, m: Metric) =>
  m === "cost" ? d.cost : m === "output" ? d.output : m === "input" ? inputOf(d) : inputOf(d) + d.output;

/** The model families with something to show for `metric`, stacking order (an unpriced model has no Cost). */
export const presentFamilies = (daily: DailyModelTokens[], metric: Metric) =>
  FAMILIES.filter((f) => daily.some((d) => family(d.model) === f && metricValue(d, metric) > 0));

/** The two bars per day of the "all" view; `color` is a theme key. */
export const IO_SERIES = [
  { label: "输入", color: "ioInput", value: inputOf },
  { label: "输出", color: "ioOutput", value: (d: DailyModelTokens) => d.output },
] as const;

export function parseDay(day: string): Date {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d);
}
export function dayKey(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}
const dayLabel = (day: string) => {
  const d = parseDay(day);
  return `${d.getMonth() + 1}月${d.getDate()}日`;
};

/** Every calendar day from the first to the last element of `days` (sorted, YYYY-MM-DD). */
export function fillDays(days: string[]): string[] {
  if (!days.length) return [];
  const out: string[] = [];
  const end = parseDay(days[days.length - 1]);
  for (let d = parseDay(days[0]); d <= end; d.setDate(d.getDate() + 1)) out.push(dayKey(d));
  return out;
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

export function dailyOption(daily: DailyModelTokens[], metric: Metric, t: Theme): ChartOption {
  const days = fillDays([...new Set(daily.map((d) => d.day))].sort());
  const present = presentFamilies(daily, metric);
  const formatValue = metric === "cost" ? formatCost : formatTokens;
  // "all": input and output side by side; otherwise one bar per day stacked by model family.
  const series =
    metric === "all"
      ? IO_SERIES.map((s) => ({
          name: s.label,
          type: "bar",
          barMaxWidth: 10,
          barGap: "15%",
          itemStyle: { color: t[s.color], borderRadius: [3, 3, 0, 0] },
          emphasis: { focus: "series" },
          data: days.map((day) => daily.filter((d) => d.day === day).reduce((sum, d) => sum + s.value(d), 0)),
        }))
      : present.map((f, i) => ({
          name: FAMILY_LABEL[f],
          type: "bar",
          stack: "t",
          barMaxWidth: 16,
          itemStyle: {
            color: t.series[f],
            borderColor: t.surface,
            borderWidth: 1,
            borderRadius: i === present.length - 1 ? [4, 4, 0, 0] : 0,
          },
          emphasis: { focus: "series" },
          data: days.map((day) =>
            daily.filter((d) => d.day === day && family(d.model) === f).reduce((s, d) => s + metricValue(d, metric), 0),
          ),
        }));
  return {
    ...base(t),
    grid: { left: 40, right: 8, top: 8, bottom: 22 },
    tooltip: {
      ...base(t).tooltip,
      trigger: "axis",
      axisPointer: { type: "shadow", shadowStyle: { color: t.grid, opacity: 0.4 } },
      formatter: (ps: { axisValue: string; marker: string; seriesName: string; value: number }[]) =>
        `<b>${dayLabel(ps[0].axisValue)}</b><br/>` +
        ps
          .filter((p) => p.value > 0)
          .reverse()
          .map((p) => `${p.marker}${p.seriesName}　${formatValue(p.value)}`)
          .join("<br/>"),
    },
    xAxis: {
      type: "category",
      data: days,
      axisTick: { show: false },
      axisLine: { lineStyle: { color: t.axis } },
      axisLabel: { color: t.muted, formatter: dayLabel, hideOverlap: true },
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

const CN_WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
const EN_WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const EN_MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Bar width shown on the activity card. */
export const BUCKET_LABEL: Record<ActivityBucket, string> = {
  hour1: "1h",
  hour3: "3h",
  hour6: "6h",
  hour12: "12h",
  day: "1d",
  week: "1w",
  month: "1mo",
};

const hourly = (b: ActivityBucket) => b.startsWith("hour");

/**
 * Axis label of the bar starting at `ms` (local time), "" for unlabeled bars: 1h/3h bars mark each
 * midnight (`Mon 20`), 6h/12h bars each Monday midnight (`Oct 5`), day and week bars each new month
 * (`Aug`), month bars each quarter; January shows the year.
 */
export function activityAxisLabel(bucket: ActivityBucket, ms: number): string {
  const d = new Date(ms);
  if (hourly(bucket)) {
    if (d.getHours() !== 0) return "";
    if (bucket === "hour1" || bucket === "hour3") return `${EN_WEEKDAYS[d.getDay()]} ${d.getDate()}`;
    return d.getDay() === 1 ? `${EN_MONTHS[d.getMonth()]} ${d.getDate()}` : "";
  }
  // Days: the 1st itself; weeks: the first Monday of the month; months: each quarter, so years stay legible.
  const first = bucket === "week" ? d.getDate() <= 7 : d.getDate() === 1;
  if (!first || (bucket === "month" && d.getMonth() % 3 !== 0)) return "";
  return d.getMonth() === 0 ? String(d.getFullYear()) : EN_MONTHS[d.getMonth()]!;
}

const cnDay = (d: Date) => `${d.getMonth() + 1}月${d.getDate()}日`;
const hm = (d: Date) => `${String(d.getHours()).padStart(2, "0")}:00`;

/** Tooltip title of a bar `[start, end)`, in Chinese. */
export function activityTitle(bucket: ActivityBucket, startMs: number, endMs: number): string {
  const s = new Date(startMs);
  const e = new Date(endMs);
  if (hourly(bucket)) return `${cnDay(s)} ${CN_WEEKDAYS[s.getDay()]} ${hm(s)}–${e.getHours() === 0 ? "24:00" : hm(e)}`;
  if (bucket === "day") return `${cnDay(s)} ${CN_WEEKDAYS[s.getDay()]}`;
  if (bucket === "week") return `${cnDay(s)}–${cnDay(new Date(endMs - 1))}`;
  return `${s.getFullYear()}年${s.getMonth() + 1}月`;
}

/** Messages per bar over time; the bar width comes with the data. */
export function activityOption(activity: Activity, t: Theme): ChartOption {
  const { bucket, bars } = activity;
  return {
    ...base(t),
    grid: { left: 40, right: 8, top: 8, bottom: 22 },
    tooltip: {
      ...base(t).tooltip,
      trigger: "axis",
      axisPointer: { type: "shadow", shadowStyle: { color: t.grid, opacity: 0.4 } },
      formatter: (ps: { dataIndex: number }[]) => {
        const b = bars[ps[0]!.dataIndex]!;
        return `<b>${activityTitle(bucket, b.startMs, b.endMs)}</b><br/>${b.messages} 条消息`;
      },
    },
    xAxis: {
      type: "category",
      data: bars.map((b) => String(b.startMs)),
      axisTick: { show: false },
      axisLine: { lineStyle: { color: t.axis } },
      axisLabel: {
        color: t.muted,
        interval: 0,
        hideOverlap: true,
        formatter: (v: string) => activityAxisLabel(bucket, Number(v)),
      },
    },
    yAxis: {
      type: "value",
      splitNumber: 2,
      minInterval: 1,
      axisLabel: { color: t.muted },
      splitLine: { lineStyle: { color: t.grid } },
    },
    series: [
      {
        type: "bar",
        barCategoryGap: bars.length > 100 ? "15%" : "25%",
        itemStyle: { color: t.series.opus, borderRadius: [2, 2, 0, 0] },
        emphasis: { itemStyle: { color: t.ioOutput } },
        data: bars.map((b) => b.messages),
      },
    ],
  };
}
