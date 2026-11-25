import { useMemo } from "react";
import type { SessionUsage } from "@/ipc/bindings";
import { formatCost } from "@/lib/format";
import { buildDaySessionsView } from "./daySessions";

const HOUR_TICKS = ["0 时", "6 时", "12 时", "18 时"] as const;
const LANE_GRID = "grid grid-cols-[150px_minmax(0,1fr)_60px] items-center gap-2.5 text-[12px]";

/**
 * Right column (top) of the single-day stats view: per-session 0–24h activity timeline sorted by Cost (top 6 + rest).
 */
export function DaySessionsPanel({
  sessions,
  dayStartMs,
  dayEndMs,
  onOpenSession,
}: {
  sessions: SessionUsage[];
  dayStartMs: number;
  dayEndMs: number;
  onOpenSession: (sessionId: string) => void;
}) {
  const view = useMemo(() => buildDaySessionsView(sessions, dayStartMs, dayEndMs), [sessions, dayStartMs, dayEndMs]);

  return (
    <section
      data-testid="day-sessions"
      aria-label="Session"
      className="flex min-h-0 min-w-0 flex-col gap-2 rounded-[10px] border border-border bg-ground px-3.5 py-3"
    >
      <div className="flex items-center gap-3">
        <h2 className="m-0 flex-1 text-[13px] font-semibold">Session</h2>
        {view.subtitle && (
          <span data-testid="day-sessions-subtitle" className="truncate text-[11px] text-secondary tabular-nums">
            {view.subtitle}
          </span>
        )}
      </div>

      {view.total === 0 ? (
        <div className="flex min-h-16 items-center justify-center text-[12px] text-secondary">这一天没有请求</div>
      ) : (
        <div className="flex flex-col gap-1">
          <div className={`-mx-1.5 px-1.5 ${LANE_GRID}`}>
            <span />
            <div className="grid grid-cols-4 text-[10px] text-secondary">
              {HOUR_TICKS.map((t) => (
                <span key={t}>{t}</span>
              ))}
            </div>
            <span className="text-right text-[10px] text-secondary">Cost</span>
          </div>

          {view.rows.map((row) => (
            <button
              key={row.sessionId}
              type="button"
              data-testid="day-session-row"
              data-session-id={row.sessionId}
              onClick={() => onOpenSession(row.sessionId)}
              className={`-mx-1.5 rounded px-1.5 py-0.5 text-left outline-none hover:bg-code focus-visible:ring-2 focus-visible:ring-accent ${LANE_GRID}`}
            >
              <span title={row.title} className="truncate">
                {row.title}
              </span>
              <span className="relative block h-3 overflow-hidden rounded-[3px] bg-code" aria-hidden>
                {row.segments.map((seg, i) => (
                  <i
                    key={i}
                    style={{ left: `${seg.leftPct.toFixed(3)}%`, width: `${seg.widthPct.toFixed(3)}%` }}
                    className="absolute top-0 bottom-0 block min-w-[3px] rounded-[3px] bg-accent"
                  />
                ))}
              </span>
              <span className="text-right text-secondary tabular-nums">{formatCost(row.cost)}</span>
            </button>
          ))}

          {view.restCount > 0 && (
            <div data-testid="day-sessions-rest" className={`-mx-1.5 px-1.5 py-0.5 ${LANE_GRID}`}>
              <span className="truncate text-secondary">其余 {view.restCount} 个</span>
              <span />
              <span className="text-right text-secondary tabular-nums">{formatCost(view.restCost)}</span>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
