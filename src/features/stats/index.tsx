import { X } from "lucide-react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { useMemo, useState } from "react";
import type { Drill } from "@/ipc/bindings";
import { Dialog, PanelDialogContent } from "@/components/ui/dialog";
import { formatCost, formatTokens } from "@/lib/format";
import { useStats } from "@/queries";
import { useUi } from "@/state/ui";
import { Bar, Card, Empty, Overview, Segmented } from "./Cards";
import { Chart, useDark } from "./Chart";
import {
  FAMILY_LABEL,
  IO_SERIES,
  type Metric,
  type ModelFamily,
  family,
  presentFamilies,
  dailyOption,
  activityOption,
  BUCKET_LABEL,
  themes,
} from "./charts";
import { DEFAULT_RANGE, resolveRange, type RangeSel } from "./range";
import { RangePicker } from "./RangePicker";

/** Stats as a large dialog over the window; a click outside or Esc closes it. */
export function StatsDialog() {
  const open = useUi((s) => s.statsOpen);
  const setOpen = useUi((s) => s.setStatsOpen);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <PanelDialogContent
        overlayTestId="stats-overlay"
        aria-describedby={undefined}
        onOpenAutoFocus={(e) => e.preventDefault()}
        className="top-1/2 h-[min(900px,calc(100vh-64px))] w-[min(1280px,calc(100vw-64px))] -translate-y-1/2 outline-none"
      >
        <StatsPanel />
      </PanelDialogContent>
    </Dialog>
  );
}

function StatsPanel() {
  const dark = useDark();
  const theme = themes[dark ? "dark" : "light"];
  const [range, setRange] = useState<RangeSel>(DEFAULT_RANGE);
  const [metric, setMetric] = useState<Metric>("all");

  const weeklyReset = useUi((s) => s.weeklyReset);
  const resolved = useMemo(() => resolveRange(range, new Date(), weeklyReset), [range, weeklyReset]);
  const { timeRange, days: rangeDays } = resolved;
  const { data: stats, isPending, isError } = useStats({ timeRange });

  const drill = (d: Drill) => {
    const ui = useUi.getState();
    ui.setProjectIds([]);
    ui.setStatsDrill(d, timeRange);
    ui.selectSession(null);
    ui.setStatsOpen(false);
  };
  const openSession = (id: string) => {
    const ui = useUi.getState();
    ui.setProjectIds([]);
    ui.setStatsDrill(null);
    ui.selectSession(id);
    ui.setStatsOpen(false);
  };

  const dailyOpt = useMemo(() => (stats ? dailyOption(stats.daily, metric, theme) : null), [stats, metric, theme]);
  const activityOpt = useMemo(() => (stats ? activityOption(stats.activity, theme) : null), [stats, theme]);

  const empty = stats != null && stats.overview.sessions === 0 && stats.daily.length === 0;
  const present = stats ? presentFamilies(stats.daily, metric) : [];
  const maxCost = Math.max(0.01, ...(stats?.sessions.map((s) => s.cost) ?? []));

  return (
    <div data-testid="stats" className="@container flex h-full min-w-0 flex-col bg-list">
      {/* The header stays put; only the content below scrolls. */}
      <header className="flex shrink-0 flex-col border-b border-border px-6">
        {/* The toolbar row keeps its height whatever the range: picker after the title, dates beside it. */}
        <div className="flex min-h-[52px] flex-wrap items-center gap-3">
          <DialogPrimitive.Title asChild>
            <h1 className="m-0 shrink-0 text-[16px] font-semibold">统计</h1>
          </DialogPrimitive.Title>
          <RangePicker sel={range} onChange={setRange} />
          {/* The cycle's reset is set in Settings › 通用. */}
          <span data-testid="cycle-span" className="min-w-0 flex-1 truncate text-[12px] text-secondary">
            {resolved.detail}
          </span>
          <DialogPrimitive.Close
            aria-label="关闭统计"
            className="-mr-2 flex size-7 items-center justify-center rounded-md text-secondary outline-none hover:bg-text/8 focus-visible:outline-2 focus-visible:outline-accent"
          >
            <X size={16} strokeWidth={1.6} aria-hidden />
          </DialogPrimitive.Close>
        </div>
      </header>

      <div data-testid="stats-scroll" className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-6 pt-4 pb-6">
        {isPending && <Empty text="加载中…" />}
        {isError && <Empty text="统计加载失败" />}
        {empty && <Empty />}

        {stats && !empty && (
          <>
            <Overview stats={stats} rangeDays={rangeDays} />
            {/* Each card is as tall as its content, not stretched to the tallest in its row. */}
            <div className="grid grid-cols-1 items-start gap-3 @3xl:grid-cols-3">
              <Card
                className="@3xl:col-span-2"
                title={metric === "all" ? "每日用量" : "每日用量 · 按模型"}
                testId="stats-daily"
                aside={
                  <>
                    {metric === "all" &&
                      IO_SERIES.map((s) => (
                        <span key={s.label} className="flex items-center gap-1 text-[11px]">
                          <span className="size-[9px] rounded-[2px]" style={{ background: theme[s.color] }} />
                          {s.label}
                        </span>
                      ))}
                    {metric !== "all" &&
                      present.map((f) => (
                        <button
                          key={f}
                          type="button"
                          data-testid="legend-item"
                          title="查看该模型的 Session"
                          onClick={() =>
                            drill({ kind: "model", model: stats.daily.find((d) => family(d.model) === f)!.model })
                          }
                          className="flex items-center gap-1 text-[11px] hover:underline"
                        >
                          <span
                            className="size-[9px] rounded-[2px]"
                            style={{ background: theme.series[f as ModelFamily] }}
                          />
                          {FAMILY_LABEL[f]}
                        </button>
                      ))}
                    <Segmented
                      small
                      label="指标"
                      value={metric}
                      options={[
                        { key: "all", label: "全部" },
                        { key: "input", label: "输入" },
                        { key: "output", label: "输出" },
                        { key: "cost", label: "Cost" },
                      ]}
                      onChange={setMetric}
                    />
                  </>
                }
              >
                {stats.daily.length === 0 ? (
                  <Empty />
                ) : (
                  <Chart
                    testId="daily-chart"
                    height={240}
                    label="每日用量堆叠柱状图"
                    option={dailyOpt!}
                    onPick={(p) => p.name && drill({ kind: "day", day: p.name })}
                  />
                )}
              </Card>

              <Card
                className="@3xl:row-span-2"
                title="Session 排行"
                aside={<span className="text-[11px] text-secondary">按 Cost</span>}
              >
                {stats.sessions.length === 0 && <Empty />}
                {stats.sessions.map((s) => (
                  <button
                    key={s.sessionId}
                    type="button"
                    data-testid="session-rank-row"
                    title={s.title}
                    onClick={() => openSession(s.sessionId)}
                    className="flex flex-col gap-[3px] rounded text-left outline-none hover:bg-code focus-visible:ring-2 focus-visible:ring-accent"
                  >
                    <span className="flex gap-2 text-[12px]">
                      <span className="min-w-0 flex-1 truncate">{s.title}</span>
                      <span className="shrink-0 text-secondary tabular-nums">
                        {formatCost(s.cost)} · {formatTokens(s.outputTokens)}
                      </span>
                    </span>
                    <Bar pct={(s.cost / maxCost) * 100} color={theme.series.opus} />
                  </button>
                ))}
              </Card>

              <Card
                className="@3xl:col-span-2"
                title="活跃时段"
                testId="stats-activity"
                aside={
                  stats.activity.bars.length > 0 && (
                    <span
                      data-testid="activity-bucket"
                      title="每根柱子的时长"
                      className="rounded border border-border px-1.5 text-[10px] text-secondary tabular-nums"
                    >
                      {BUCKET_LABEL[stats.activity.bucket]}
                    </span>
                  )
                }
              >
                {stats.activity.bars.length === 0 ? (
                  <Empty />
                ) : (
                  <Chart
                    testId="activity-chart"
                    height={160}
                    label="各时间段的消息数柱状图"
                    option={activityOpt!}
                    onPick={(p) => {
                      const bar = p.dataIndex == null ? undefined : stats.activity.bars[p.dataIndex];
                      if (bar) drill({ kind: "span", fromMs: bar.startMs, toMs: bar.endMs });
                    }}
                  />
                )}
              </Card>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
