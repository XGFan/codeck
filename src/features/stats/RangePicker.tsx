import { ChevronDown, ChevronLeft, ChevronRight } from "lucide-react";
import { Fragment } from "react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useUi } from "@/state/ui";
import { pageable, RANGE_MENU, resolveRange, sameRange, type RangeSel } from "./range";

const ARROW =
  "flex size-7 items-center justify-center rounded-md border border-border bg-ground text-text outline-none hover:bg-selection focus-visible:outline-2 focus-visible:outline-accent disabled:text-secondary/50 disabled:hover:bg-ground";

/**
 * The time range: a dropdown of cycles, months, the last N days and everything, then ‹ › stepping a cycle or
 * month back and forth (to the current one at most; the rolling ranges do not step). The dropdown is as wide as its
 * longest label, so the arrows stay under the pointer while its text changes.
 */
export function RangePicker({ sel, onChange }: { sel: RangeSel; onChange: (s: RangeSel) => void }) {
  const weeklyReset = useUi((s) => s.weeklyReset);
  const r = resolveRange(sel, new Date(), weeklyReset);
  const steps = pageable(sel.kind);
  const unit = sel.kind === "month" ? "个月" : "周期";
  return (
    <div role="group" aria-label="时间范围" className="flex items-center gap-1.5">
      <DropdownMenu>
        <DropdownMenuTrigger
          data-testid="range-trigger"
          className="flex h-7 w-[156px] items-center justify-between gap-1 rounded-md border border-border bg-ground px-2.5 text-[13px] font-semibold outline-none hover:bg-selection focus-visible:outline-2 focus-visible:outline-accent"
        >
          <span className="min-w-0 truncate">{r.label}</span>
          <ChevronDown size={13} strokeWidth={1.8} className="shrink-0" aria-hidden />
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
