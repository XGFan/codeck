import { RefreshCw } from "lucide-react";
import { cn } from "@/lib/cn";
import { usePlanUsage } from "./hooks";
import { buildMeterView, formatPlanBadge, formatUpdatedAt } from "./model";
import { QuotaMeter, QuotaMeterSkeleton } from "./QuotaMeter";

const LOADING_LABELS = ["当前 5 小时", "本周 · 全部模型", "本周 · 按模型"];

/** Top subscription quota strip inside the stats panel. Hidden entirely when the account has no subscription quota. */
export function QuotaStrip({ now }: { now: Date }) {
  const { state, isFetching, refresh } = usePlanUsage();

  if (state.kind === "none") return null;

  if (state.kind === "loading") {
    return (
      <section
        data-testid="quota-strip"
        data-state="loading"
        aria-label="订阅额度"
        className="flex flex-col gap-3 rounded-[10px] border border-border bg-ground px-[14px] py-[12px] @2xl:flex-row @2xl:items-stretch @2xl:gap-5"
      >
        <div className="flex shrink-0 flex-col justify-center gap-1.5 @2xl:w-[96px]">
          <h2 className="m-0 text-[13px] font-semibold">额度</h2>
        </div>
        <div className="grid min-w-0 flex-1 grid-cols-1 gap-4 @xl:grid-cols-3 @2xl:gap-6">
          {LOADING_LABELS.map((label) => (
            <QuotaMeterSkeleton key={label} label={label} />
          ))}
        </div>
      </section>
    );
  }

  if (state.kind === "failed") {
    return (
      <section
        data-testid="quota-strip"
        data-state="failed"
        aria-label="订阅额度"
        className="flex flex-col gap-2 rounded-[10px] border border-border bg-ground px-[14px] py-[12px] @2xl:flex-row @2xl:items-stretch @2xl:gap-5"
      >
        <div className="flex shrink-0 flex-col justify-center gap-1.5 @2xl:w-[96px]">
          <h2 className="m-0 text-[13px] font-semibold">额度</h2>
        </div>
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">
          <span className="text-[12px] text-text">读不到订阅额度</span>
          <span data-testid="quota-error" className="min-w-0 text-[11px] text-secondary">
            {state.error}
          </span>
        </div>
        <div className="flex shrink-0 items-center">
          <button
            type="button"
            data-testid="quota-retry"
            disabled={isFetching}
            onClick={refresh}
            className="rounded text-[12px] text-accent outline-none hover:underline focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50"
          >
            重试
          </button>
        </div>
      </section>
    );
  }

  const badge = formatPlanBadge(state.usage.plan);
  const nowMs = now.getTime();
  const colsClass =
    state.usage.limits.length <= 1
      ? "@xl:grid-cols-1"
      : state.usage.limits.length === 2
        ? "@xl:grid-cols-2"
        : "@xl:grid-cols-3";

  return (
    <section
      data-testid="quota-strip"
      data-state="ok"
      aria-label="订阅额度"
      className="flex flex-col gap-3 rounded-[10px] border border-border bg-ground px-[14px] py-[12px] @2xl:flex-row @2xl:items-stretch @2xl:gap-5"
    >
      <div className="flex shrink-0 flex-row items-center gap-2 @2xl:w-[96px] @2xl:flex-col @2xl:items-start @2xl:justify-center @2xl:gap-1.5">
        <h2 className="m-0 text-[13px] font-semibold">额度</h2>
        {badge && (
          <span
            data-testid="quota-plan"
            className="inline-flex h-[18px] items-center rounded border border-border bg-ground px-1.5 text-[11px] text-secondary"
          >
            {badge}
          </span>
        )}
      </div>
      <div className={cn("grid min-w-0 flex-1 grid-cols-1 gap-4 @2xl:gap-6", colsClass)}>
        {state.usage.limits.map((limit, idx) => (
          <QuotaMeter key={`${limit.kind}:${limit.model ?? ""}:${idx}`} view={buildMeterView(limit, nowMs, "strip")} />
        ))}
      </div>
      <div className="flex shrink-0 flex-row items-center justify-between gap-2 @2xl:flex-col @2xl:items-end @2xl:justify-center @2xl:gap-0.5">
        <button
          type="button"
          data-testid="quota-refresh"
          aria-label="刷新订阅额度"
          title="刷新订阅额度"
          disabled={isFetching}
          onClick={refresh}
          className="flex size-6 items-center justify-center rounded-md text-secondary outline-none hover:bg-text/8 focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-50"
        >
          <RefreshCw size={14} strokeWidth={1.8} className={cn(isFetching && "animate-spin")} aria-hidden />
        </button>
        <span
          data-testid="quota-updated"
          title={state.error ?? undefined}
          className={cn("text-[11px] whitespace-nowrap", state.error ? "text-error" : "text-secondary")}
        >
          {formatUpdatedAt(state.usage.fetchedMs, state.error)}
        </span>
      </div>
    </section>
  );
}
