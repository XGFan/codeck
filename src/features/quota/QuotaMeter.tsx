import { cn } from "@/lib/cn";
import type { MeterView, QuotaTone } from "./model";

const BAR_COLOR: Record<QuotaTone, string> = {
  normal: "bg-accent",
  warn: "bg-live-waiting",
  full: "bg-error",
};

const TEXT_COLOR: Record<QuotaTone, string> = {
  normal: "text-text",
  warn: "text-tag-text",
  full: "text-error",
};

/** One rate-limit row: label + percentage, progress bar with optional elapsed tick, and reset/status caption. */
export function QuotaMeter({ view }: { view: MeterView }) {
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
      <span role="img" aria-label={view.ariaLabel} className="relative block h-[6px] rounded-[3px] bg-border">
        <span
          className={cn("block h-full max-w-full rounded-[3px]", BAR_COLOR[view.tone])}
          style={{ width: `${view.fillPct}%` }}
        />
        {view.elapsedPct != null && (
          <span
            data-testid="quota-tick"
            className="absolute top-[-3px] h-[12px] w-[2px] -translate-x-1/2 rounded-[1px] bg-text"
            style={{ left: `${view.elapsedPct}%` }}
          />
        )}
      </span>
      <span
        data-testid="quota-subtext"
        title={view.subtext || undefined}
        className={cn(
          "min-h-[15px] truncate text-[11px]",
          view.tone === "normal" ? "text-secondary" : TEXT_COLOR[view.tone],
        )}
      >
        {view.subtext || "\u00A0"}
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
