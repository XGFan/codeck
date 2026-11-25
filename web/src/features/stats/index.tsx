import { X } from "lucide-react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { useMemo, useState } from "react";
import type { Drill, Stats } from "@/ipc/bindings";
import { Dialog, PanelDialogContent } from "@/components/ui/dialog";
import { QuotaStrip, useCycleRule, useMinuteNow, usePlanUsage } from "@/features/quota";
import { useStats } from "@/queries";
import { useUi } from "@/state/ui";
import { useDark } from "./Chart";
import { themes } from "./charts";
import { DaySessionsPanel } from "./DaySessionsPanel";
import { FrequencyPanel } from "./FrequencyPanel";
import {
  dayOffset,
  DEFAULT_RANGE,
  resolveDayCompareRange,
  resolvePrev7Range,
  resolveRange,
  type DayCompare,
  type RangeSel,
  type ResolvedRange,
} from "./range";
import { RangePicker } from "./RangePicker";
import { loadDayCompare, saveDayCompare, usablePrevStats } from "./trend";
import { TrendPanel } from "./TrendPanel";
import { UsagePanel } from "./UsagePanel";

const DAY_MS = 86_400_000;

interface PrevSnapshot {
  stats: Stats;
  startMs: number;
  endMs: number;
}

interface StatsSnapshot {
  stats: Stats;
  range: RangeSel;
  resolved: ResolvedRange;
  prev: PrevSnapshot | null;
  prev7: Stats | null;
  dayCompare: DayCompare;
}

function Empty({ text = "所选范围内没有数据" }: { text?: string }) {
  return <div className="flex min-h-16 flex-1 items-center justify-center text-[12px] text-secondary">{text}</div>;
}

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
        className="top-1/2 h-[min(720px,calc(100vh-64px))] w-[min(1180px,calc(100vw-64px))] -translate-y-1/2 outline-none"
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
  const [dayCompare, setDayCompare] = useState<DayCompare>(loadDayCompare);

  const now = useMinuteNow();
  const { rule } = useCycleRule();
  const { state: planState } = usePlanUsage();
  const resolved = useMemo(() => resolveRange(range, now, rule), [range, now, rule]);
  const { timeRange, startMs, endMs } = resolved;

  const { data: stats, isPending, isError, isPlaceholderData } = useStats({ timeRange });

  const wantPrevQuery = range.kind === "day" || range.kind === "cycle" || range.kind === "last7";
  const prevBounds = useMemo(() => {
    if (!wantPrevQuery || startMs == null || endMs == null) return null;
    if (range.kind === "day") return resolveDayCompareRange(range.offset, dayCompare, now, rule);
    if (range.kind === "cycle") {
      const prev = resolveRange({ kind: "cycle", offset: range.offset - 1 }, now, rule);
      return prev.timeRange && prev.startMs != null && prev.endMs != null
        ? { timeRange: prev.timeRange, startMs: prev.startMs, endMs: prev.endMs }
        : null;
    }
    const prevStart = startMs - 7 * DAY_MS;
    const prevEnd = startMs;
    return { timeRange: { fromMs: prevStart, toMs: prevEnd - 1 }, startMs: prevStart, endMs: prevEnd };
  }, [wantPrevQuery, range, dayCompare, now, rule, startMs, endMs]);

  const prevQuery = useStats(
    { timeRange: prevBounds?.timeRange ?? null },
    { enabled: wantPrevQuery && prevBounds != null, keepPrevious: false },
  );
  const prevStats = usablePrevStats(prevQuery);
  const currentPrev: PrevSnapshot | null =
    prevStats && prevBounds ? { stats: prevStats, startMs: prevBounds.startMs, endMs: prevBounds.endMs } : null;

  const wantPrev7Query = range.kind === "day";
  const prev7Bounds = useMemo(
    () => (wantPrev7Query ? resolvePrev7Range(range.offset, now) : null),
    [wantPrev7Query, range.offset, now],
  );
  const prev7Query = useStats(
    { timeRange: prev7Bounds?.timeRange ?? null },
    { enabled: wantPrev7Query && prev7Bounds != null, keepPrevious: false },
  );
  const currentPrev7 = usablePrevStats(prev7Query) ?? null;

  // Pair placeholder data with its own range + reference snapshot so the previous view stays intact until new data arrives.
  const [settled, setSettled] = useState<StatsSnapshot | null>(null);
  if (
    stats &&
    !isPlaceholderData &&
    (settled?.stats !== stats ||
      settled.resolved !== resolved ||
      settled.prev?.stats !== currentPrev?.stats ||
      settled.prev?.startMs !== currentPrev?.startMs ||
      settled.prev7 !== currentPrev7 ||
      settled.dayCompare !== dayCompare)
  ) {
    setSettled({ stats, range, resolved, prev: currentPrev, prev7: currentPrev7, dayCompare });
  }
  const shown: StatsSnapshot | null =
    isPlaceholderData && settled
      ? settled
      : stats
        ? { stats, range, resolved, prev: currentPrev, prev7: currentPrev7, dayCompare }
        : null;

  const drill = (d: Drill) => {
    if (isPlaceholderData) return;
    const ui = useUi.getState();
    ui.setProjectIds([]);
    ui.setStatsDrill(d, timeRange);
    ui.selectSession(null);
    ui.setStatsOpen(false);
  };

  const selectDay = (day: string) => {
    if (isPlaceholderData) return;
    const offset = dayOffset(day, now);
    if (offset > 0) return;
    setRange({ kind: "day", offset });
  };

  const openSession = (id: string) => {
    if (isPlaceholderData) return;
    const ui = useUi.getState();
    ui.setProjectIds([]);
    ui.setStatsDrill(null);
    ui.selectSession(id);
    ui.setStatsOpen(false);
  };

  const changeDayCompare = (next: DayCompare) => {
    if (isPlaceholderData || next === dayCompare) return;
    saveDayCompare(next);
    setDayCompare(next);
  };

  const shownDay = shown?.range.kind === "day";
  const empty = shown != null && !shownDay && shown.stats.daily.length === 0;
  const shownTrend =
    shown != null && (shown.range.kind === "day" || shown.range.kind === "cycle" || shown.range.kind === "last7");

  return (
    <div
      data-testid="stats"
      data-placeholder={isPlaceholderData ? "true" : undefined}
      className="@container flex h-full min-w-0 flex-col bg-list"
    >
      {/* The header stays put; only the content below scrolls. */}
      <header className="flex shrink-0 flex-col border-b border-border px-6">
        <div className="flex min-h-[52px] flex-wrap items-center gap-3">
          <DialogPrimitive.Title asChild>
            <h1 className="m-0 shrink-0 text-[16px] font-semibold">统计</h1>
          </DialogPrimitive.Title>
          <RangePicker sel={range} now={now} rule={rule} onChange={setRange} />
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

      <div data-testid="stats-scroll" className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-6 pt-4 pb-6">
        <QuotaStrip now={now} />
        {isPending && <Empty text="加载中…" />}
        {isError && <Empty text="统计加载失败" />}
        {empty && <Empty />}

        {shown && !empty && (
          <div className="grid grid-cols-1 items-start gap-3 @3xl:grid-cols-2">
            <UsagePanel
              stats={shown.stats}
              isDay={shownDay}
              prev7Stats={shown.prev7}
              timeRange={shown.resolved.timeRange}
              startMs={shown.resolved.startMs}
              endMs={shown.resolved.endMs}
              now={now}
              theme={theme}
              onDrill={drill}
              onSelectDay={selectDay}
            />
            <div className="flex min-w-0 flex-col gap-3">
              {shownDay && shown.resolved.startMs != null && shown.resolved.endMs != null ? (
                <DaySessionsPanel
                  sessions={shown.stats.sessions}
                  dayStartMs={shown.resolved.startMs}
                  dayEndMs={shown.resolved.endMs}
                  onOpenSession={openSession}
                />
              ) : (
                <FrequencyPanel
                  stats={shown.stats}
                  timeRange={shown.resolved.timeRange}
                  startMs={shown.resolved.startMs}
                  endMs={shown.resolved.endMs}
                  now={now}
                  theme={theme}
                  onSelectDay={selectDay}
                />
              )}
              {shownTrend && shown.resolved.startMs != null && shown.resolved.endMs != null && (
                <TrendPanel
                  sel={shown.range}
                  stats={shown.stats}
                  prevStats={shown.prev?.stats}
                  rangeStartMs={shown.resolved.startMs}
                  rangeEndMs={shown.resolved.endMs}
                  prevStartMs={shown.prev?.startMs ?? null}
                  prevEndMs={shown.prev?.endMs ?? null}
                  now={now}
                  planState={planState}
                  isPlaceholderData={isPlaceholderData}
                  theme={theme}
                  dayCompare={shown.dayCompare}
                  onDayCompareChange={shownDay ? changeDayCompare : undefined}
                />
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
