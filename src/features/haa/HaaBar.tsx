import { ChevronDown, ChevronRight } from "lucide-react";
import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";
import { formatTokens } from "@/lib/format";
import { useUi } from "@/state/ui";
import { HAA_KEY } from "@/features/transcript/keys";
import type { HaaWorker, HaaWorkerState } from "./protocol";
import { useHaa } from "./store";
import {
  formatCollapsedSummary,
  formatTaskCell,
  formatTaskTooltip,
  formatWorkerDuration,
  isActiveState,
  isActiveWorker,
  shortModelName,
  sortWorkers,
  STATE_LABEL,
} from "./view";

const GRID_CLASS =
  "grid min-w-0 items-center gap-x-[12px] grid-cols-[108px_52px_72px_minmax(0,1fr)_128px_52px_52px] @max-[760px]:grid-cols-[108px_52px_72px_minmax(0,1fr)_52px_52px]";

const DOT_CLASS: Record<HaaWorkerState, string> = {
  running: "bg-live",
  asking: "border-[1.5px] border-live bg-transparent",
  taken: "bg-secondary/60",
  idle: "bg-secondary/50",
  dormant: "border-[1.5px] border-secondary/60 bg-transparent",
  dead: "bg-error",
};

const STATE_TEXT_CLASS: Record<HaaWorkerState, string> = {
  running: "text-live",
  asking: "text-secondary",
  taken: "text-secondary",
  idle: "text-secondary",
  dormant: "text-secondary",
  dead: "text-error",
};

/** Current timestamp, ticking every `intervalMs` while non-null. */
function useNow(intervalMs: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (intervalMs == null) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(t);
  }, [intervalMs]);
  return now;
}

/** Small 12px ring showing context usage percentage (matches Connect Header's ring). */
function CtxRing({ pct }: { pct: number }) {
  const f = Math.min(1, Math.max(0, pct / 100));
  const c = 2 * Math.PI * 9;
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" className="shrink-0" aria-hidden>
      <circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" strokeOpacity="0.2" strokeWidth="3" />
      <circle
        cx="12"
        cy="12"
        r="9"
        fill="none"
        stroke="currentColor"
        strokeWidth="3"
        strokeDasharray={`${f * c} ${c}`}
        transform="rotate(-90 12 12)"
      />
    </svg>
  );
}

function ToggleButton({
  activeCount,
  total,
  open,
  onToggle,
}: {
  activeCount: number;
  total: number;
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      data-testid="haa-toggle"
      aria-expanded={open}
      title={`${total} 个 Worker，${activeCount} 个在干活`}
      onClick={onToggle}
      className="inline-flex max-w-full min-w-0 cursor-pointer items-center gap-[4px] overflow-hidden rounded-sm text-left whitespace-nowrap text-secondary hover:text-text focus:outline-none focus-visible:ring-1 focus-visible:ring-accent"
    >
      <span className="shrink-0 font-mono text-[11px] font-medium">haa</span>
      <span className={cn("truncate text-[11px] tabular-nums", activeCount > 0 && "text-live")}>
        {activeCount}/{total}
      </span>
      {open ? (
        <ChevronDown size={12} className="shrink-0" aria-hidden />
      ) : (
        <ChevronRight size={12} className="shrink-0" aria-hidden />
      )}
    </button>
  );
}

function WorkerRow({ w, now }: { w: HaaWorker; now: number }) {
  const active = isActiveState(w.state);
  const taskCell = formatTaskCell(w);
  const taskTooltip = formatTaskTooltip(w);
  const modelShort = shortModelName(w.model);

  return (
    <div data-testid="haa-worker" data-state={w.state} className={cn(GRID_CLASS, "h-[24px] overflow-hidden")}>
      <div className="flex min-w-0 items-center gap-[6px] overflow-hidden whitespace-nowrap">
        <span className={cn("size-[6px] shrink-0 rounded-full", DOT_CLASS[w.state])} aria-hidden />
        <span
          className={cn("min-w-0 truncate", active ? "font-medium text-text" : "font-normal text-secondary")}
          title={[w.name, [w.agent, w.model].filter(Boolean).join(" · ")].filter(Boolean).join("\n")}
        >
          {w.name}
        </span>
      </div>
      <div className={cn("min-w-0 truncate", STATE_TEXT_CLASS[w.state])}>{STATE_LABEL[w.state]}</div>
      <div className="min-w-0 truncate text-secondary tabular-nums">{formatWorkerDuration(w, now)}</div>
      <div
        className={cn("min-w-0 truncate", taskCell.muted ? "text-secondary" : "text-text")}
        title={taskTooltip || undefined}
      >
        {taskCell.prefix && <span className="font-medium text-secondary">{taskCell.prefix}</span>}
        {taskCell.text}
      </div>
      <div className="min-w-0 truncate text-right text-secondary @max-[760px]:hidden" title={w.model}>
        {modelShort}
      </div>
      {w.ctx != null ? (
        <div
          className="flex min-w-0 items-center justify-end gap-[4px] overflow-hidden whitespace-nowrap text-secondary tabular-nums"
          title="上下文占用"
        >
          <CtxRing pct={w.ctx} />
          <span className="truncate">{Math.round(w.ctx)}%</span>
        </div>
      ) : (
        <div className="min-w-0 truncate text-right text-secondary">—</div>
      )}
      <div className="min-w-0 truncate text-right text-secondary tabular-nums" title="累计 token">
        {w.tokens > 0 ? formatTokens(w.tokens) : "—"}
      </div>
    </div>
  );
}

/**
 * Read-only aligned status table for the haa Workers owned by `sessionId`.
 * Renders nothing when the Session has no haa Workers.
 */
export function HaaBar({ sessionId }: { sessionId: string | null }) {
  const workers = useHaa((s) => s.workers);
  const own = sessionId ? workers.filter((w) => w.owner === sessionId) : [];

  // Kept with the open Session's view (collapsed again when another is opened), not here: the bar is mounted anew
  // when the Session is Connected or Disconnected, which moves it into or out of the Connect panel.
  const open = useUi((s) => s.expanded[HAA_KEY] ?? false);
  const toggleExpanded = useUi((s) => s.toggleExpanded);
  const toggle = () => toggleExpanded(HAA_KEY);

  const total = own.length;
  const collapsible = total > 2;
  const activeCount = own.filter(isActiveWorker).length;
  const collapsedSummaryOnly = collapsible && !open && activeCount === 0;

  const visibleRaw = collapsedSummaryOnly ? [] : collapsible && !open ? own.filter(isActiveWorker) : own;
  const hasFastTick = visibleRaw.some((w) => (w.state === "running" || w.state === "asking") && w.startedAt != null);
  const hasSlowTick =
    !hasFastTick && visibleRaw.some((w) => w.state !== "running" && w.state !== "asking" && w.lastActive != null);
  const tickInterval = hasFastTick ? 1000 : hasSlowTick ? 30_000 : null;
  const now = useNow(tickInterval);

  if (total === 0) return null;

  const visible = collapsedSummaryOnly ? [] : sortWorkers(visibleRaw, now);

  return (
    <div
      data-testid="haa-workers"
      data-haa-bar=""
      className="@container flex w-full min-w-0 flex-col overflow-x-hidden rounded-2xl border border-border bg-ground px-3.5 py-2 text-[12px] shadow-[0_2px_10px_rgba(0,0,0,0.04)]"
    >
      {collapsedSummaryOnly ? (
        <div className="flex h-[24px] min-w-0 items-center gap-[12px] overflow-hidden">
          <div className="flex min-w-0 shrink-0 items-center">
            <ToggleButton activeCount={activeCount} total={total} open={open} onToggle={toggle} />
          </div>
          <span data-testid="haa-summary" className="min-w-0 truncate text-[11px] text-secondary">
            {formatCollapsedSummary(own)}
          </span>
        </div>
      ) : (
        <>
          <div className={cn(GRID_CLASS, "h-[26px] border-b border-border text-[11px] text-secondary")}>
            <div className="flex min-w-0 items-center overflow-hidden">
              {collapsible ? (
                <ToggleButton activeCount={activeCount} total={total} open={open} onToggle={toggle} />
              ) : (
                <div
                  data-testid="haa-count"
                  className="flex min-w-0 items-center gap-[6px] overflow-hidden whitespace-nowrap"
                >
                  <span className="shrink-0 font-mono text-[11px] font-medium text-secondary">haa</span>
                  <span
                    className={cn(
                      "truncate text-[11px] tabular-nums",
                      activeCount > 0 ? "text-live" : "text-secondary",
                    )}
                  >
                    {activeCount}/{total}
                  </span>
                </div>
              )}
            </div>
            <div className="min-w-0 truncate">状态</div>
            <div className="min-w-0 truncate">时长</div>
            <div className="min-w-0 truncate">Task</div>
            <div className="min-w-0 truncate text-right @max-[760px]:hidden">模型</div>
            <div className="min-w-0 truncate text-right">上下文</div>
            <div className="min-w-0 truncate text-right">token</div>
          </div>
          <div className="max-h-[45vh] overflow-x-hidden overflow-y-auto">
            {visible.map((w) => (
              <WorkerRow key={w.id} w={w} now={now} />
            ))}
          </div>
        </>
      )}
    </div>
  );
}
