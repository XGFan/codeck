import { useMemo, useState } from "react";
import type { Drill, Stats, TimeRange } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCost, formatCount, formatTokens } from "@/lib/format";
import { Chart } from "./Chart";
import { FAMILY_LABEL, usageBarOption, type Theme } from "./charts";
import { buildUsageSeries, elapsedDays, inputTotals, modelMetricBreakdown, type UsageMetric } from "./usage";

const MODEL_TITLE: Record<UsageMetric, string> = {
  cost: "Cost 按模型",
  input: "输入按模型",
  output: "输出按模型",
};

const INPUT_PARTS = [
  {
    key: "read" as const,
    label: "缓存读取",
    className: "bg-accent",
    hint: "已在缓存中的前缀（系统提示词、之前的对话），约按 0.1 倍输入单价计费",
  },
  {
    key: "write" as const,
    label: "缓存写入",
    className: "bg-accent/45",
    hint: "本次写入缓存、下一轮可按缓存读取复用的内容，按 1.25 倍（5 分钟）或 2 倍（1 小时）输入单价计费",
  },
  {
    key: "input" as const,
    label: "未缓存输入",
    className: "bg-text/20",
    hint: "既未读取也未写入缓存的输入，按原价计费",
  },
];

/** Left column of the stats dialog: Cost / input / output switcher, stacked bar chart, Cost by model, input composition. */
export function UsagePanel({
  stats,
  timeRange,
  startMs,
  endMs,
  now,
  theme,
  onDrill,
}: {
  stats: Stats;
  timeRange: TimeRange | null;
  startMs: number | null;
  endMs: number | null;
  now: Date;
  theme: Theme;
  onDrill: (d: Drill) => void;
}) {
  const [metric, setMetric] = useState<UsageMetric>("cost");
  const nowMs = now.getTime();
  const { overview, daily } = stats;

  const days = useMemo(() => elapsedDays(startMs, endMs, daily, nowMs), [startMs, endMs, daily, nowMs]);
  const totals = useMemo(() => inputTotals(daily), [daily]);
  const totalInput = totals.input + totals.read + totals.write;

  const series = useMemo(() => buildUsageSeries(daily, metric, timeRange, nowMs), [daily, metric, timeRange, nowMs]);
  const barOption = useMemo(() => usageBarOption(series, metric, theme), [series, metric, theme]);
  const modelRows = useMemo(() => modelMetricBreakdown(daily, metric), [daily, metric]);
  const modelTitle = MODEL_TITLE[metric];
  const formatMetric = metric === "cost" ? formatCost : formatTokens;

  const pct = (v: number) => (totalInput > 0 ? (v / totalInput) * 100 : 0);
  const figures: {
    key: UsageMetric;
    label: string;
    value: string;
    avg: string;
    extra?: string;
  }[] = [
    {
      key: "cost",
      label: "Cost",
      value: formatCost(overview.cost),
      avg: `日均 ${formatCost(overview.cost / days)}`,
      extra:
        overview.unpricedMessages > 0 ? `${formatCount(overview.unpricedMessages)} 条消息无价格，未计入` : undefined,
    },
    {
      key: "input",
      label: "输入",
      value: formatTokens(totalInput),
      avg: `日均 ${formatTokens(Math.round(totalInput / days))}`,
    },
    {
      key: "output",
      label: "输出",
      value: formatTokens(overview.outputTokens),
      avg: `日均 ${formatTokens(Math.round(overview.outputTokens / days))}`,
    },
  ];

  const chartLabel = `${series.bucket === "1d" ? "每日" : series.bucket === "1w" ? "每周" : "每月"} ${
    metric === "cost" ? "Cost" : metric === "input" ? "输入" : "输出"
  }，按模型堆叠`;

  return (
    <section
      data-testid="stats-usage"
      aria-label="用量"
      className="flex min-h-0 min-w-0 flex-col gap-3 rounded-[10px] border border-border bg-ground px-3.5 py-3"
    >
      <div className="flex items-center gap-3">
        <h2 className="m-0 flex-1 text-[13px] font-semibold">用量</h2>
        <span
          data-testid="usage-bucket"
          title="每根柱子的时长"
          className="rounded border border-border px-1.5 text-[10px] text-secondary tabular-nums"
        >
          {series.bucket}
        </span>
      </div>

      <div className="flex flex-wrap items-start gap-x-9 gap-y-2">
        {figures.map((f) => {
          const active = metric === f.key;
          return (
            <button
              key={f.key}
              type="button"
              data-testid={`metric-${f.key}`}
              aria-pressed={active}
              onClick={() => setMetric(f.key)}
              className={cn(
                "flex flex-col gap-0.5 border-b-2 pb-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-accent",
                active ? "border-accent" : "border-transparent hover:border-border",
              )}
            >
              <span className="text-[12px] text-secondary">{f.label}</span>
              <span className="text-[24px] leading-[1.25] font-semibold tracking-tight tabular-nums">{f.value}</span>
              <span className="text-[11px] text-secondary tabular-nums">{f.avg}</span>
              {f.extra && (
                <span data-testid="unpriced-note" className="text-[11px] text-secondary">
                  {f.extra}
                </span>
              )}
            </button>
          );
        })}
      </div>

      <Chart
        testId="usage-chart"
        height={160}
        label={chartLabel}
        option={barOption}
        onPick={(p) => {
          if (series.bucket !== "1d") return;
          const day = p.dataIndex != null ? series.items[p.dataIndex]?.key : p.name;
          if (day) onDrill({ kind: "day", day });
        }}
      />

      {modelRows.length > 0 && (
        <div data-testid="model-cost" className="flex flex-col gap-[7px]">
          <span data-testid="model-cost-title" className="text-[12px] text-secondary">
            {modelTitle}
          </span>
          <div
            role="img"
            aria-label={`${modelTitle}：${modelRows.map((r) => `${FAMILY_LABEL[r.family]} ${r.sharePct}%`).join("，")}`}
            className="flex h-[6px] overflow-hidden rounded-[3px] bg-border"
          >
            {modelRows.map((r) => (
              <span key={r.family} style={{ width: `${r.shareExact}%`, background: theme.series[r.family] }} />
            ))}
          </div>
          <div className="flex flex-col gap-0.5">
            {modelRows.map((r) => (
              <button
                key={r.family}
                type="button"
                data-testid="model-cost-row"
                data-family={r.family}
                title={`查看 ${FAMILY_LABEL[r.family]} 的 Session`}
                onClick={() => onDrill({ kind: "model", models: r.models })}
                className="-mx-1.5 flex items-center gap-2 rounded px-1.5 py-0.5 text-left text-[12px] outline-none hover:bg-code focus-visible:ring-2 focus-visible:ring-accent"
              >
                <span className="size-[9px] shrink-0 rounded-[2px]" style={{ background: theme.series[r.family] }} />
                <span className="min-w-0 flex-1 truncate">{FAMILY_LABEL[r.family]}</span>
                <span className="shrink-0 text-secondary tabular-nums">
                  {formatMetric(r.value)} · {r.sharePct}%
                </span>
              </button>
            ))}
          </div>
        </div>
      )}

      <div className="h-px bg-border" />

      <div data-testid="input-breakdown" className="flex flex-col gap-1.5">
        <div className="flex items-baseline gap-2">
          <span className="flex-1 text-[12px] text-secondary">输入构成</span>
          <span data-testid="cache-hit" className="text-[12px] text-secondary">
            缓存命中率 <span className="font-semibold text-text tabular-nums">{pct(totals.read).toFixed(1)}%</span>
          </span>
        </div>
        <div
          role="img"
          aria-label={`输入构成：${INPUT_PARTS.map((p) => `${p.label} ${Math.round(pct(totals[p.key]))}%`).join("，")}`}
          className="flex h-[6px] overflow-hidden rounded-[3px] bg-border"
        >
          {INPUT_PARTS.map((p) => (
            <span key={p.key} className={p.className} style={{ width: `${pct(totals[p.key])}%` }} />
          ))}
        </div>
        <div className="flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-secondary">
          {INPUT_PARTS.map((p) => (
            <span key={p.key} title={p.hint} className="flex cursor-default items-center gap-1 whitespace-nowrap">
              <span className={cn("size-[9px] shrink-0 rounded-[2px]", p.className)} />
              {p.label} {formatTokens(totals[p.key])}
            </span>
          ))}
        </div>
      </div>
    </section>
  );
}
