import { Fragment, useCallback, useMemo, useState } from "react";
import type { Stats, TimeRange } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCount } from "@/lib/format";
import type { Theme } from "./charts";
import { FreqTip, type FreqTipTarget } from "./FreqTip";
import {
  buildHourlyGrid,
  buildMonthBlocks,
  chooseFrequencyMode,
  WEEKDAYS_MON,
  type HeatLevel,
  type HourlyGrid,
  type MonthBlock,
  type MonthBlocksView,
} from "./frequency";

const LEVEL_BG: Record<HeatLevel, string> = {
  0: "bg-code",
  1: "bg-accent/18 dark:bg-accent/22",
  2: "bg-accent/38 dark:bg-accent/42",
  3: "bg-accent/65 dark:bg-accent/70",
  4: "bg-accent",
};

const HOUR_GROUPS = ["0–6 时", "6–12 时", "12–18 时", "18–24 时"] as const;

const BLOCK_DENSITY = {
  1: { blockGap: "gap-1.5", gridGap: "gap-1", cell: "h-[34px] rounded-[6px]", blank: "h-[34px]" },
  2: { blockGap: "gap-1", gridGap: "gap-[3px]", cell: "h-[22px] rounded-[4px]", blank: "h-[22px]" },
  4: { blockGap: "gap-1", gridGap: "gap-[2px]", cell: "h-[12px] rounded-[2px]", blank: "h-[12px]" },
} as const;

interface ScopedTip extends FreqTipTarget {
  scope: unknown;
}

function Legend({ caption }: { caption: string }) {
  return (
    <div className="flex items-center gap-1.5 text-[10px] text-secondary">
      <span>少</span>
      {([0, 1, 2, 3, 4] as const).map((lv) => (
        <i key={lv} className={cn("block size-[10px] rounded-[2px]", LEVEL_BG[lv])} aria-hidden />
      ))}
      <span>多</span>
      <span className="flex-1" />
      <span className="truncate">{caption}</span>
    </div>
  );
}

function HourlyGridSection({ grid, theme }: { grid: HourlyGrid; theme: Theme }) {
  const [tip, setTip] = useState<ScopedTip | null>(null);
  const dismiss = useCallback(() => setTip(null), []);
  const activeTip = tip && tip.scope === grid ? tip : null;

  return (
    <>
      <div
        data-testid="freq-hourly"
        role="img"
        aria-label="按小时的请求数：一行一天，一格一小时"
        onMouseLeave={dismiss}
        className="flex flex-col gap-[2px]"
      >
        <div className="flex items-center gap-2">
          <span className="w-[26px] shrink-0" />
          {HOUR_GROUPS.map((g) => (
            <span key={g} className="flex-1 text-[10px] text-secondary">
              {g}
            </span>
          ))}
        </div>
        {grid.rows.map((row) => (
          <div key={row.day} data-testid="freq-hour-row" className="flex items-center gap-2">
            <span className="w-[26px] shrink-0 text-[10px] whitespace-nowrap text-secondary">{row.weekday}</span>
            {[0, 6, 12, 18].map((offset) => (
              <span key={offset} className="grid flex-1 grid-cols-6 gap-[2px]">
                {row.cells
                  .slice(offset, offset + 6)
                  .map((c) =>
                    c.kind === "out" ? (
                      <i key={c.hour} data-testid="freq-hour-out" className="block aspect-square" aria-hidden />
                    ) : (
                      <i
                        key={c.hour}
                        data-testid="freq-hour"
                        data-level={c.level}
                        onMouseEnter={(e) =>
                          setTip({ anchor: e.currentTarget, title: c.tipTitle, requests: c.requests, scope: grid })
                        }
                        onMouseLeave={dismiss}
                        className={cn("block aspect-square rounded-[3px]", LEVEL_BG[c.level])}
                      />
                    ),
                  )}
              </span>
            ))}
          </div>
        ))}
      </div>
      <FreqTip tip={activeTip} theme={theme} onDismiss={dismiss} />
      <Legend caption="一格一小时，颜色是请求数" />
    </>
  );
}

function MonthBlockCard({
  block,
  columns,
  showWeekdays,
  onSelectDay,
  onHover,
  onLeave,
}: {
  block: MonthBlock;
  columns: 1 | 2 | 4;
  showWeekdays: boolean;
  onSelectDay: (day: string) => void;
  onHover: (anchor: HTMLElement, title: string, requests: number) => void;
  onLeave: () => void;
}) {
  const style = BLOCK_DENSITY[columns];
  return (
    <div data-testid="freq-month-block" className={cn("flex min-w-0 flex-col", style.blockGap)}>
      <span className="text-[11px] text-secondary">{block.label}</span>
      {showWeekdays && (
        <div data-testid="freq-weekdays" className="grid grid-cols-7 gap-1 text-center text-[10px] text-secondary">
          {WEEKDAYS_MON.map((w) => (
            <span key={w}>{w}</span>
          ))}
        </div>
      )}
      <div className={cn("grid grid-cols-7", style.gridGap)}>
        {Array.from({ length: block.leadBlanks }).map((_, i) => (
          <i key={`lead-${i}`} className={cn("block", style.blank)} aria-hidden />
        ))}
        {block.days.map((c) => {
          if (!c.inRange || c.future) {
            return <i key={c.day} className={cn("block", style.blank)} aria-hidden />;
          }
          return (
            <button
              key={c.day}
              type="button"
              data-testid="freq-day"
              data-day={c.day}
              data-level={c.level}
              data-today={c.isToday ? "true" : undefined}
              aria-label={`${c.tipTitle} · ${formatCount(c.requests)} 次请求`}
              onMouseEnter={(e) => onHover(e.currentTarget, c.tipTitle, c.requests)}
              onMouseLeave={onLeave}
              onClick={() => onSelectDay(c.day)}
              className={cn(
                "block outline-none focus-visible:ring-2 focus-visible:ring-accent",
                style.cell,
                LEVEL_BG[c.level],
                c.isToday && "ring-[1.5px] ring-inset ring-text",
              )}
            />
          );
        })}
      </div>
    </div>
  );
}

function MonthBlocksSection({
  view,
  theme,
  onSelectDay,
}: {
  view: MonthBlocksView;
  theme: Theme;
  onSelectDay: (day: string) => void;
}) {
  const [tip, setTip] = useState<ScopedTip | null>(null);
  const dismiss = useCallback(() => setTip(null), []);
  const hover = useCallback(
    (anchor: HTMLElement, title: string, requests: number) => setTip({ anchor, title, requests, scope: view }),
    [view],
  );
  const activeTip = tip && tip.scope === view ? tip : null;

  return (
    <>
      {view.groupedByYear ? (
        <div data-testid="freq-months" data-columns="4" onMouseLeave={dismiss} className="flex flex-col gap-3">
          {view.years.map((yr) => (
            <div key={yr.year} data-testid="freq-year-group" className="flex flex-col gap-2">
              <span data-testid="freq-year" className="text-[12px] font-semibold text-text tabular-nums">
                {yr.year}
              </span>
              <div className="grid grid-cols-4 gap-x-5 gap-y-3.5">
                {yr.blocks.map((b) => (
                  <MonthBlockCard
                    key={`${b.year}-${b.month}`}
                    block={b}
                    columns={4}
                    showWeekdays={false}
                    onSelectDay={onSelectDay}
                    onHover={hover}
                    onLeave={dismiss}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div
          data-testid="freq-months"
          data-columns={view.columns}
          onMouseLeave={dismiss}
          className={view.columns === 1 ? "flex flex-col" : "grid grid-cols-2 gap-x-7 gap-y-4"}
        >
          {view.years.map((yr) => (
            <Fragment key={yr.year}>
              {yr.blocks.map((b) => (
                <MonthBlockCard
                  key={`${b.year}-${b.month}`}
                  block={b}
                  columns={view.columns}
                  showWeekdays={view.columns === 1}
                  onSelectDay={onSelectDay}
                  onHover={hover}
                  onLeave={dismiss}
                />
              ))}
            </Fragment>
          ))}
        </div>
      )}
      <FreqTip tip={activeTip} theme={theme} onDismiss={dismiss} />
      <Legend caption={view.columns === 1 ? "一格一天，颜色是请求数" : "每块一个月，列是周一到周日，一格一天"} />
    </>
  );
}

/**
 * Right column (top): 「使用频率」 colored by request count (5 levels: 0 + non-zero quartiles).
 * - Hourly day-by-day grid (7–8 rows × 24 hours) when `hourly` is non-empty (cycles & last 7 days, no drill);
 * - One block per calendar month otherwise (1 col with weekday header for 1 month, 2 cols for 2–4, 4 cols by year for >4; click day to open that day).
 */
export function FrequencyPanel({
  stats,
  timeRange,
  startMs,
  endMs,
  now,
  theme,
  onSelectDay,
}: {
  stats: Stats;
  timeRange: TimeRange | null;
  startMs: number | null;
  endMs: number | null;
  now: Date;
  theme: Theme;
  onSelectDay: (day: string) => void;
}) {
  const nowMs = now.getTime();
  const mode = chooseFrequencyMode(stats.hourly.length);

  const hourlyGrid = useMemo(
    () => (mode === "hourly" ? buildHourlyGrid(stats.hourly, startMs, endMs, nowMs) : null),
    [mode, stats.hourly, startMs, endMs, nowMs],
  );
  const monthBlocks = useMemo(
    () => (mode === "months" ? buildMonthBlocks(stats.daily, timeRange, nowMs) : null),
    [mode, stats.daily, timeRange, nowMs],
  );

  const subtitle = mode === "hourly" ? (hourlyGrid?.subtitle ?? "") : (monthBlocks?.summary.subtitle ?? "");

  return (
    <section
      data-testid="stats-frequency"
      data-mode={mode}
      aria-label="使用频率"
      className="flex min-h-0 min-w-0 flex-col gap-2.5 rounded-[10px] border border-border bg-ground px-3.5 py-3"
    >
      <div className="flex items-center gap-3">
        <h2 className="m-0 flex-1 text-[13px] font-semibold">使用频率</h2>
        <span data-testid="freq-subtitle" className="truncate text-[11px] text-secondary tabular-nums">
          {subtitle}
        </span>
      </div>

      {mode === "hourly" && hourlyGrid && <HourlyGridSection grid={hourlyGrid} theme={theme} />}
      {mode === "months" && monthBlocks && (
        <MonthBlocksSection view={monthBlocks} theme={theme} onSelectDay={onSelectDay} />
      )}
    </section>
  );
}
