import { ChevronDown, ChevronLeft, ChevronRight, X } from "lucide-react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { Fragment, type ReactNode, useMemo, useState } from "react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Drill, Stats } from "@/ipc/bindings";
import { Dialog, PanelDialogContent } from "@/components/ui/dialog";
import { cn } from "@/lib/cn";
import { formatCost, formatCount, formatTokens } from "@/lib/format";
import { useStats } from "@/queries";
import { useUi } from "@/state/ui";
import { Chart, useDark } from "./Chart";
import {
  FAMILIES,
  FAMILY_LABEL,
  IO_SERIES,
  type Metric,
  type ModelFamily,
  family,
  dailyOption,
  activityOption,
  BUCKET_LABEL,
  themes,
} from "./charts";
import { DEFAULT_RANGE, pageable, RANGE_MENU, resolveRange, sameRange, type RangeSel } from "./range";

const DAY = 86_400_000;

function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  small,
}: {
  label: string;
  value: T;
  options: { key: T; label: string }[];
  onChange: (k: T) => void;
  small?: boolean;
}) {
  return (
    <div role="group" aria-label={label} className="flex overflow-hidden rounded-md border border-border bg-ground">
      {options.map((o, i) => (
        <button
          key={o.key}
          type="button"
          aria-pressed={value === o.key}
          onClick={() => onChange(o.key)}
          className={cn(
            "px-3 text-[12px] outline-none focus-visible:ring-2 focus-visible:ring-accent",
            small ? "h-6 px-2 text-[11px]" : "h-7",
            i > 0 && "border-l border-border",
            // No bold for the active option: a wider label would shift the right-aligned group.
            value === o.key ? "bg-selection text-accent" : "text-secondary hover:bg-code",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function Card({
  title,
  aside,
  className,
  children,
  testId,
}: {
  title: ReactNode;
  aside?: ReactNode;
  className?: string;
  children: ReactNode;
  testId?: string;
}) {
  return (
    <section
      data-testid={testId}
      className={cn(
        "flex min-h-0 min-w-0 flex-col gap-2 rounded-[10px] border border-border bg-ground px-3.5 py-3",
        className,
      )}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h2 className="m-0 flex-1 text-[13px] font-semibold">{title}</h2>
        {aside}
      </div>
      {children}
    </section>
  );
}

const Empty = ({ text = "所选范围内没有数据" }: { text?: string }) => (
  <div className="flex min-h-16 flex-1 items-center justify-center text-[12px] text-secondary">{text}</div>
);

function longestStreak(days: string[]): number {
  const sorted = [...new Set(days)].sort();
  let best = 0;
  let run = 0;
  let prev = 0;
  for (const d of sorted) {
    const [y, m, dd] = d.split("-").map(Number);
    const t = Date.UTC(y, m - 1, dd);
    run = prev && t - prev === DAY ? run + 1 : 1;
    prev = t;
    best = Math.max(best, run);
  }
  return best;
}

const CARD = "flex min-w-0 flex-col gap-0.5 rounded-[10px] border border-border bg-ground px-3.5 py-3";

/** Input (new, cache writes, cache reads) and output; the bar shows what the input was made of. */
function TokenCard({ stats }: { stats: Stats }) {
  const t = stats.daily.reduce(
    (a, d) => ({ input: a.input + d.input, read: a.read + d.cacheRead, write: a.write + d.cacheCreation }),
    { input: 0, read: 0, write: 0 },
  );
  const totalIn = t.input + t.read + t.write;
  const pct = (v: number) => (totalIn ? (v / totalIn) * 100 : 0);
  const parts = [
    {
      label: "缓存读取",
      value: t.read,
      className: "bg-accent",
      hint: "已在缓存中的前缀（系统提示词、之前的对话），约按 0.1 倍输入单价计费",
    },
    {
      label: "缓存写入",
      value: t.write,
      className: "bg-accent/45",
      hint: "本次写入缓存、下一轮可按缓存读取复用的内容，按 1.25 倍（5 分钟）或 2 倍（1 小时）输入单价计费",
    },
    { label: "未缓存输入", value: t.input, className: "bg-text/20", hint: "既未读取也未写入缓存的输入，按原价计费" },
  ];
  return (
    <div data-testid="stat-card" data-card="tokens" className={cn(CARD, "col-span-2 gap-[5px]")}>
      <div className="flex items-baseline gap-2">
        <span className="text-[12px] text-secondary">Token</span>
        <span className="flex-1" />
        <span data-testid="cache-hit" className="text-[12px] text-secondary">
          缓存命中率 <span className="font-semibold text-text">{Math.round(pct(t.read))}%</span>
        </span>
      </div>
      {/* Two fixed columns: a longer number on the left never pushes the output figure around. */}
      <div className="grid grid-cols-2 gap-3">
        <div data-testid="token-input" className="flex min-w-0 items-baseline gap-1.5">
          <span className="text-[24px] leading-tight font-semibold tracking-tight">{formatTokens(totalIn)}</span>
          <span className="text-[12px] text-secondary">输入</span>
        </div>
        <div data-testid="token-output" className="flex min-w-0 items-baseline gap-1.5">
          <span className="text-[24px] leading-tight font-semibold tracking-tight">
            {formatTokens(stats.overview.outputTokens)}
          </span>
          <span className="text-[12px] text-secondary">输出</span>
        </div>
      </div>
      <div
        role="img"
        aria-label={`输入构成：${parts.map((p) => `${p.label} ${Math.round(pct(p.value))}%`).join("，")}`}
        className="flex h-1.5 overflow-hidden rounded-full bg-border"
      >
        {parts.map((p) => (
          <span key={p.label} className={p.className} style={{ width: `${pct(p.value)}%` }} />
        ))}
      </div>
      <div className="flex flex-wrap gap-x-3 text-[11px] text-secondary">
        {parts.map((p) => (
          <span key={p.label} title={p.hint} className="flex cursor-default items-center gap-1 whitespace-nowrap">
            <span className={cn("size-2 rounded-[2px]", p.className)} />
            {p.label} {formatTokens(p.value)}
          </span>
        ))}
      </div>
    </div>
  );
}

/** A daily average: one decimal below 10, so a quiet range does not read as 0. */
const perDay = (v: number) => (v < 10 ? String(Math.round(v * 10) / 10) : formatCount(Math.round(v)));

function Overview({ stats, rangeDays }: { stats: Stats; rangeDays: number | null }) {
  const { overview: o, daily } = stats;
  const activeDays = daily.map((d) => d.day);
  const span = rangeDays ?? Math.max(o.activeDays, stats.heatDaily.length);
  const cards = [
    {
      id: "sessions",
      label: "Session",
      value: formatCount(o.sessions),
      sub: o.activeDays ? `日均 ${perDay(o.sessions / o.activeDays)} 个` : "—",
    },
    {
      id: "messages",
      label: "消息",
      value: formatCount(o.messages),
      sub: o.activeDays ? `日均 ${formatCount(Math.round(o.messages / o.activeDays))} 条` : "—",
    },
    {
      id: "cost",
      label: "Cost",
      value: formatCost(o.cost),
      sub:
        o.unpricedMessages > 0
          ? `${formatCount(o.unpricedMessages)} 条消息无价格，未计入`
          : o.activeDays
            ? `日均 ${formatCost(o.cost / o.activeDays)}`
            : "—",
    },
    {
      id: "active",
      label: "活跃天数",
      value: `${o.activeDays} / ${span}`,
      sub: activeDays.length ? `最长连续 ${longestStreak(activeDays)} 天` : "—",
    },
  ];
  return (
    <div className="grid grid-cols-2 gap-3 @2xl:grid-cols-4 @5xl:grid-cols-6">
      {cards.map((c, i) => (
        <Fragment key={c.id}>
          {/* Token sits third, after Session and messages. */}
          {i === 2 && <TokenCard stats={stats} />}
          <div data-testid="stat-card" data-card={c.id} className={CARD}>
            <span className="text-[12px] text-secondary">{c.label}</span>
            <span className="text-[24px] leading-tight font-semibold tracking-tight">{c.value}</span>
            <span className="truncate text-[11px] text-secondary">{c.sub}</span>
          </div>
        </Fragment>
      ))}
    </div>
  );
}

function Bar({ pct, color, className }: { pct: number; color: string; className?: string }) {
  return (
    <span className={cn("block h-1.5 rounded-[3px] bg-code", className)}>
      <span className="block h-1.5 rounded-[3px]" style={{ width: `${Math.max(2, pct)}%`, background: color }} />
    </span>
  );
}

const ARROW =
  "flex size-7 items-center justify-center rounded-md border border-border bg-ground text-text outline-none hover:bg-selection focus-visible:outline-2 focus-visible:outline-accent disabled:text-secondary/50 disabled:hover:bg-ground";

/**
 * The time range: a dropdown of cycles, months, the last N days and everything, then ‹ › stepping a cycle or
 * month back and forth (to the current one at most; the rolling ranges do not step).
 */
function RangePicker({ sel, onChange }: { sel: RangeSel; onChange: (s: RangeSel) => void }) {
  const weeklyReset = useUi((s) => s.weeklyReset);
  const r = resolveRange(sel, new Date(), weeklyReset);
  const steps = pageable(sel.kind);
  const unit = sel.kind === "month" ? "个月" : "周期";
  return (
    <div role="group" aria-label="时间范围" className="flex items-center gap-1.5">
      <DropdownMenu>
        <DropdownMenuTrigger
          data-testid="range-trigger"
          className="flex h-7 items-center gap-1 rounded-md border border-border bg-ground px-2.5 text-[13px] font-semibold outline-none hover:bg-selection focus-visible:outline-2 focus-visible:outline-accent"
        >
          {r.label}
          <ChevronDown size={13} strokeWidth={1.8} aria-hidden />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="min-w-48">
          {RANGE_MENU.map((g, gi) => (
            <Fragment key={g.group ?? gi}>
              {gi > 0 && <DropdownMenuSeparator />}
              {g.group && <div className="px-2 pt-1 pb-0.5 text-[11px] text-secondary">{g.group}</div>}
              {g.items.map((it) => (
                <DropdownMenuCheckboxItem
                  key={it.label}
                  checked={sameRange(it.sel, sel)}
                  onCheckedChange={() => onChange(it.sel)}
                >
                  {it.label}
                </DropdownMenuCheckboxItem>
              ))}
            </Fragment>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <button
        type="button"
        aria-label={`上一${unit}`}
        disabled={!steps}
        onClick={() => onChange({ ...sel, offset: sel.offset - 1 })}
        className={ARROW}
      >
        <ChevronLeft size={14} strokeWidth={1.8} aria-hidden />
      </button>
      <button
        type="button"
        aria-label={`下一${unit}`}
        disabled={!steps || sel.offset >= 0}
        onClick={() => onChange({ ...sel, offset: sel.offset + 1 })}
        className={ARROW}
      >
        <ChevronRight size={14} strokeWidth={1.8} aria-hidden />
      </button>
    </div>
  );
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
  const present = stats ? FAMILIES.filter((f) => stats.daily.some((d) => family(d.model) === f)) : [];
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
                title={metric === "all" ? "每日 token" : "每日 token · 按模型"}
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
                    label="每日 token 堆叠柱状图"
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
