import { ChartColumn, RefreshCw } from "lucide-react";
import { Popover } from "radix-ui";
import { useState } from "react";
import { StatsToggle, TOOL_CLASS } from "@/features/layout";
import type { PlanUsage } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { useUi } from "@/state/ui";
import { useMinuteNow, usePlanUsage } from "./hooks";
import { buildMeterView, buildRingView, formatPlanBadge, formatUpdatedAt, type QuotaTone } from "./model";
import { QuotaMeter } from "./QuotaMeter";

const RING_STROKE: Record<QuotaTone, string> = {
  normal: "stroke-accent",
  warn: "stroke-live-waiting",
  full: "stroke-error",
};

function QuotaPopoverBody({
  usage,
  error,
  isFetching,
  onRefresh,
  onOpenStats,
}: {
  usage: PlanUsage;
  error: string | null;
  isFetching: boolean;
  onRefresh: () => void;
  onOpenStats: () => void;
}) {
  const now = useMinuteNow();
  const badge = formatPlanBadge(usage.plan);
  const nowMs = now.getTime();

  return (
    <>
      <div className="flex items-center gap-3">
        <h2 className="m-0 flex-1 text-[13px] font-semibold">额度</h2>
        {badge && (
          <span
            data-testid="quota-plan"
            className="inline-flex h-[18px] items-center rounded border border-border bg-ground px-1.5 text-[11px] text-secondary"
          >
            {badge}
          </span>
        )}
        <button
          type="button"
          data-testid="quota-refresh"
          aria-label="刷新额度"
          title="刷新额度"
          disabled={isFetching}
          onClick={onRefresh}
          className="flex size-6 items-center justify-center rounded-md text-secondary outline-none hover:bg-text/8 focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50"
        >
          <RefreshCw size={14} strokeWidth={1.8} className={cn(isFetching && "animate-spin")} aria-hidden />
        </button>
      </div>
      {usage.limits.map((limit, idx) => (
        <QuotaMeter key={`${limit.kind}:${limit.model ?? ""}:${idx}`} view={buildMeterView(limit, nowMs, "popover")} />
      ))}
      <span
        data-testid="quota-updated"
        title={error ?? undefined}
        className={cn("truncate text-[11px]", error ? "text-error" : "text-secondary")}
      >
        {formatUpdatedAt(usage.fetchedMs, error)}
      </span>
      <button
        type="button"
        data-testid="quota-open-stats"
        onClick={onOpenStats}
        className="flex h-[30px] items-center justify-center gap-1.5 rounded-md border border-border bg-ground text-[12px] text-text outline-none hover:bg-selection focus-visible:outline-2 focus-visible:outline-accent"
      >
        <ChartColumn size={15} strokeWidth={1.7} aria-hidden />
        打开统计
      </button>
    </>
  );
}

/**
 * Foot-of-sidebar / toolbar entry: shows a circular quota ring opening the quota popover when subscription quota is
 * loaded (`ok`), or falls back to the plain stats icon (`StatsToggle`) in `loading` / `none` / `failed`.
 */
export function QuotaNavButton({ className, side = "top" }: { className?: string; side?: "top" | "bottom" }) {
  const { state, isFetching, refresh } = usePlanUsage();
  const [open, setOpen] = useState(false);

  if (state.kind !== "ok") return <StatsToggle className={className} />;

  const ring = buildRingView(state.usage);

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button
          type="button"
          data-testid="nav-quota"
          data-tone={ring.tone}
          aria-label={ring.ariaLabel}
          title={ring.ariaLabel}
          className={cn(TOOL_CLASS, "data-[state=open]:bg-selection", className)}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden>
            <circle cx="12" cy="12" r="8.5" className="stroke-text/18" strokeWidth="3" />
            {ring.fillPct > 0 && (
              <circle
                cx="12"
                cy="12"
                r="8.5"
                className={RING_STROKE[ring.tone]}
                strokeWidth="3"
                strokeLinecap="round"
                strokeDasharray={`${ring.dash.toFixed(1)} ${ring.circumference.toFixed(1)}`}
                transform="rotate(-90 12 12)"
              />
            )}
          </svg>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          data-testid="quota-popover"
          role="dialog"
          aria-label="额度"
          side={side}
          align="start"
          sideOffset={8}
          collisionPadding={8}
          className="z-50 flex w-[296px] flex-col gap-3 rounded-[10px] border border-border bg-ground p-[14px] text-text shadow-lg outline-none"
        >
          <QuotaPopoverBody
            usage={state.usage}
            error={state.error}
            isFetching={isFetching}
            onRefresh={refresh}
            onOpenStats={() => {
              setOpen(false);
              useUi.getState().setStatsOpen(true);
            }}
          />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
