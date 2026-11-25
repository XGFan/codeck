import { Fragment, useMemo } from "react";
import type { Drill, Stats, TimeRange } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCount } from "@/lib/format";
import {
  buildCalendarGrid,
  buildHourlyGrid,
  buildMonthBlocks,
  chooseFrequencyMode,
  WEEKDAYS_MON,
  type HeatLevel,
} from "./frequency";
import type { RangeKind } from "./range";
import { resolveDaySpan } from "./usage";

const LEVEL_BG: Record<HeatLevel, string> = {
  0: "bg-code",
  1: "bg-accent/18 dark:bg-accent/22",
  2: "bg-accent/38 dark:bg-accent/42",
  3: "bg-accent/65 dark:bg-accent/70",
  4: "bg-accent",
};

const CALENDAR_TEXT: Record<HeatLevel, string> = {
  0: "text-secondary",
  1: "text-text",
  2: "text-text",
  3: "text-text",
  4: "text-white",
};

const HOUR_GROUPS = ["0–6 时", "6–12 时", "12–18 时", "18–24 时"] as const;

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

/**
 * Right column (top): 「使用频率」 colored by request count (5 levels: 0 + non-zero quartiles).
 * - Hourly 7×24 grid when `hourly` is non-empty (cycles & last 7 days, no drill);
 * - Monthly calendar when `<= 31` days (months & last 30 days, click day to drill);
 * - Mini month blocks when longer (2 per row for `<= 4` blocks; 4 per row grouped by year otherwise, click day to drill).
 */
export function FrequencyPanel({
  stats,
  rangeKind,
  timeRange,
  startMs,
  endMs,
  now,
  onDrill,
}: {
  stats: Stats;
  rangeKind: RangeKind;
  timeRange: TimeRange | null;
  startMs: number | null;
  endMs: number | null;
  now: Date;
  onDrill: (d: Drill) => void;
}) {
  const nowMs = now.getTime();
  const span = useMemo(() => resolveDaySpan(timeRange, stats.daily, nowMs), [timeRange, stats.daily, nowMs]);
  const durationMs = startMs != null && endMs != null ? endMs - startMs : null;
  const mode = chooseFrequencyMode(stats.hourly.length, durationMs, span?.spanDays ?? 0);

  const hourlyGrid = useMemo(
    () => (mode === "hourly" ? buildHourlyGrid(stats.hourly, startMs, nowMs) : null),
    [mode, stats.hourly, startMs, nowMs],
  );
  const calendarGrid = useMemo(
    () => (mode === "calendar" ? buildCalendarGrid(stats.daily, timeRange, nowMs) : null),
    [mode, stats.daily, timeRange, nowMs],
  );
  const monthBlocks = useMemo(
    () => (mode === "months" ? buildMonthBlocks(stats.daily, timeRange, nowMs) : null),
    [mode, stats.daily, timeRange, nowMs],
  );

  const subtitle =
    mode === "hourly"
      ? (hourlyGrid?.subtitle ?? "")
      : mode === "calendar"
        ? (calendarGrid?.summary.subtitle ?? "")
        : (monthBlocks?.summary.subtitle ?? "");

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

      {mode === "hourly" && hourlyGrid && (
        <>
          <div
            data-testid="freq-hourly"
            role="img"
            aria-label="按小时的请求数：一行是星期几，一格一小时"
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
            {hourlyGrid.rows.map((cells, r) => (
              <div key={WEEKDAYS_MON[r]} className="flex items-center gap-2">
                <span className="w-[26px] shrink-0 text-[10px] whitespace-nowrap text-secondary">
                  {WEEKDAYS_MON[r]}
                </span>
                {[0, 6, 12, 18].map((offset) => (
                  <span key={offset} className="grid flex-1 grid-cols-6 gap-[2px]">
                    {cells.slice(offset, offset + 6).map((c) => (
                      <i
                        key={c.hour}
                        data-testid="freq-hour"
                        data-level={c.level}
                        data-future={c.future ? "true" : undefined}
                        data-start={c.isStart ? "true" : undefined}
                        title={c.title || undefined}
                        className={cn(
                          "relative block aspect-square rounded-[3px]",
                          c.future ? "bg-list ring-1 ring-inset ring-border" : LEVEL_BG[c.level],
                          c.isStart &&
                            "before:absolute before:top-[-2px] before:bottom-[-2px] before:left-[-2px] before:w-[2px] before:rounded-[1px] before:bg-text",
                        )}
                      />
                    ))}
                  </span>
                ))}
              </div>
            ))}
          </div>
          <Legend
            caption={
              rangeKind === "cycle"
                ? "一格一小时，颜色是请求数 · 竖线是周期的起止"
                : "一格一小时，颜色是请求数 · 竖线是范围起点"
            }
          />
        </>
      )}

      {mode === "calendar" && calendarGrid && (
        <>
          <div
            data-testid="freq-calendar"
            role="group"
            aria-label="按天的请求数月历：一格一天"
            className="grid grid-cols-7 gap-1"
          >
            {WEEKDAYS_MON.map((w) => (
              <span key={w} className="text-center text-[10px] text-secondary">
                {w}
              </span>
            ))}
            {calendarGrid.cells.map((c) => {
              if (!c.inRange) return <span key={c.day} className="h-[50px]" aria-hidden />;
              if (c.future) {
                return (
                  <div
                    key={c.day}
                    data-future="true"
                    className="flex h-[50px] flex-col justify-between rounded-[6px] bg-list px-[7px] py-[5px] text-secondary/50 tabular-nums ring-1 ring-inset ring-border"
                  >
                    <b className="text-[11px] font-semibold">{c.label}</b>
                    <span className="text-right text-[10px]" />
                  </div>
                );
              }
              return (
                <button
                  key={c.day}
                  type="button"
                  data-testid="freq-day"
                  data-day={c.day}
                  data-level={c.level}
                  data-today={c.isToday ? "true" : undefined}
                  title={c.title}
                  onClick={() => onDrill({ kind: "day", day: c.day })}
                  className={cn(
                    "flex h-[50px] flex-col justify-between rounded-[6px] px-[7px] py-[5px] text-left tabular-nums outline-none focus-visible:ring-2 focus-visible:ring-accent",
                    LEVEL_BG[c.level],
                    CALENDAR_TEXT[c.level],
                    c.isToday && "ring-[1.5px] ring-inset ring-text",
                  )}
                >
                  <b className="text-[11px] font-semibold">{c.label}</b>
                  <span className="text-right text-[10px]">{c.requests > 0 ? formatCount(c.requests) : ""}</span>
                </button>
              );
            })}
          </div>
          <Legend caption="一格一天，颜色与数字是请求数" />
        </>
      )}

      {mode === "months" && monthBlocks && (
        <>
          {monthBlocks.groupedByYear ? (
            <div data-testid="freq-months" data-columns="4" className="flex flex-col gap-3">
              {monthBlocks.years.map((yr, yi) => (
                <div key={yr.year} data-testid="freq-year-group" className="flex flex-col gap-2">
                  <div className="flex items-baseline gap-2">
                    <span data-testid="freq-year" className="text-[12px] font-semibold text-text tabular-nums">
                      {yr.year}
                    </span>
                    {yi === 0 && <span className="text-[10px] text-secondary">每块一个月，列是周一到周日</span>}
                  </div>
                  <div className="grid grid-cols-4 gap-x-5 gap-y-3.5">
                    {yr.blocks.map((b) => (
                      <div
                        key={`${b.year}-${b.month}`}
                        data-testid="freq-month-block"
                        className="flex min-w-0 flex-col gap-1"
                      >
                        <span className="text-[11px] text-secondary">{b.label}</span>
                        <div className="grid grid-cols-7 gap-[2px]">
                          {Array.from({ length: b.leadBlanks }).map((_, i) => (
                            <i key={`lead-${i}`} className="block h-[12px]" aria-hidden />
                          ))}
                          {b.days.map((c) => {
                            if (c.future) {
                              return (
                                <i
                                  key={c.day}
                                  data-future="true"
                                  className="block h-[12px] rounded-[2px] bg-list ring-1 ring-inset ring-border"
                                />
                              );
                            }
                            if (!c.inRange) return <i key={c.day} className="block h-[12px]" aria-hidden />;
                            return (
                              <button
                                key={c.day}
                                type="button"
                                data-testid="freq-day"
                                data-day={c.day}
                                data-level={c.level}
                                data-today={c.isToday ? "true" : undefined}
                                title={c.title}
                                aria-label={c.title}
                                onClick={() => onDrill({ kind: "day", day: c.day })}
                                className={cn(
                                  "block h-[12px] rounded-[2px] outline-none focus-visible:ring-2 focus-visible:ring-accent",
                                  LEVEL_BG[c.level],
                                  c.isToday && "ring-[1.5px] ring-inset ring-text",
                                )}
                              />
                            );
                          })}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <div data-testid="freq-months" data-columns="2" className="grid grid-cols-2 gap-x-7 gap-y-4">
              {monthBlocks.years.map((yr) => (
                <Fragment key={yr.year}>
                  {yr.blocks.map((b) => (
                    <div
                      key={`${b.year}-${b.month}`}
                      data-testid="freq-month-block"
                      className="flex min-w-0 flex-col gap-1"
                    >
                      <span className="text-[11px] text-secondary">{b.label}</span>
                      <div className="grid grid-cols-7 gap-[3px]">
                        {Array.from({ length: b.leadBlanks }).map((_, i) => (
                          <i key={`lead-${i}`} className="block h-[22px]" aria-hidden />
                        ))}
                        {b.days.map((c) => {
                          if (c.future) {
                            return (
                              <i
                                key={c.day}
                                data-future="true"
                                className="block h-[22px] rounded-[4px] bg-list ring-1 ring-inset ring-border"
                              />
                            );
                          }
                          if (!c.inRange) return <i key={c.day} className="block h-[22px]" aria-hidden />;
                          return (
                            <button
                              key={c.day}
                              type="button"
                              data-testid="freq-day"
                              data-day={c.day}
                              data-level={c.level}
                              data-today={c.isToday ? "true" : undefined}
                              title={c.title}
                              aria-label={c.title}
                              onClick={() => onDrill({ kind: "day", day: c.day })}
                              className={cn(
                                "block h-[22px] rounded-[4px] outline-none focus-visible:ring-2 focus-visible:ring-accent",
                                LEVEL_BG[c.level],
                                c.isToday && "ring-[1.5px] ring-inset ring-text",
                              )}
                            />
                          );
                        })}
                      </div>
                    </div>
                  ))}
                </Fragment>
              ))}
            </div>
          )}
          <Legend
            caption={
              monthBlocks.groupedByYear
                ? "一格一天，颜色是请求数"
                : "每块一个月，列是周一到周日，一格一天，颜色是请求数"
            }
          />
        </>
      )}
    </section>
  );
}
