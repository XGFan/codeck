import { cn } from "@/lib/cn";
import type { MeterView, QuotaTone } from "./model";

const BAR_COLOR: Record<QuotaTone, string> = {
  normal: "bg-accent",
  warn: "bg-live-waiting",
  full: "bg-error",
};

const AHEAD_COLOR: Record<QuotaTone, string> = {
  normal: "bg-quota-ahead",
  warn: "bg-quota-ahead-warn",
  full: "bg-quota-ahead-full",
};

const TEXT_COLOR: Record<QuotaTone, string> = {
  normal: "text-text",
  warn: "text-tag-text",
  full: "text-error",
};

/** One rate-limit row: label + percentage, progress bar with optional darkened ahead segment, and caption. */
export function QuotaMeter({ view }: { view: MeterView }) {
  const subtextTitle = view.aheadText
    ? view.subtext
      ? `${view.subtext} · ${view.aheadText}`
      : view.aheadText
    : view.subtext || undefined;

  return (
    <div
      data-testid="quota-meter"
      data-tone={view.tone}
      data-kind={view.kind}
      className="flex min-w-0 flex-col gap-[5px]"
    >
      <div className="flex items-baseline gap-2">
        <span className="min-w-0 flex-1 truncate text-[12px] text-secondary">{view.label}</span>
        <span className={cn("shrink-0 text-[13px] font-semibold tabular-nums", TEXT_COLOR[view.tone])}>
          {view.percent}%
        </span>
      </div>
      <span
        role="img"
        aria-label={view.ariaLabel}
        title={view.elapsedTitle ?? undefined}
        className="relative block h-[6px] overflow-hidden rounded-[3px] bg-border"
      >
        {view.isAhead && view.aheadStartPct != null ? (
          <>
            <span
              className={cn("absolute inset-y-0 left-0 block", BAR_COLOR[view.tone])}
              style={{ width: `${view.aheadStartPct}%` }}
            />
            <span
              data-testid="quota-ahead"
              className={cn("absolute inset-y-0 block", AHEAD_COLOR[view.tone])}
              style={{ left: `${view.aheadStartPct}%`, width: `${Math.max(0, view.fillPct - view.aheadStartPct)}%` }}
            />
          </>
        ) : (
          <span
            className={cn("absolute inset-y-0 left-0 block", BAR_COLOR[view.tone])}
            style={{ width: `${view.fillPct}%` }}
          />
        )}
      </span>
      <span
        data-testid="quota-subtext"
        title={subtextTitle}
        className={cn(
          "min-h-[15px] truncate text-[11px]",
          view.tone === "normal" ? "text-secondary" : TEXT_COLOR[view.tone],
        )}
      >
        {view.subtext || view.aheadText ? (
          <>
            {view.subtext}
            {view.aheadText && (
              <>
                {view.subtext ? " · " : ""}
                <i
                  className={cn("mr-1 inline-block size-[7px] rounded-[2px] align-baseline", AHEAD_COLOR[view.tone])}
                  aria-hidden
                />
                {view.aheadText}
              </>
            )}
          </>
        ) : (
          "\u00A0"
        )}
      </span>
    </div>
  );
}

/** Placeholder bar shown while subscription quota is being fetched for the first time. */
export function QuotaMeterSkeleton({ label }: { label: string }) {
  return (
    <div data-testid="quota-meter-loading" className="flex min-w-0 flex-col gap-[5px]">
      <div className="flex items-baseline gap-2">
        <span className="min-w-0 flex-1 truncate text-[12px] text-secondary">{label}</span>
        <span className="shrink-0 text-[13px] font-semibold text-secondary tabular-nums">…</span>
      </div>
      <span className="relative block h-[6px] rounded-[3px] bg-border" />
      <span className="min-h-[15px] truncate text-[11px] text-secondary">正在读取</span>
    </div>
  );
}
