import { format } from "date-fns";
import { zhCN } from "date-fns/locale";
import type { TimeRange } from "@/ipc/bindings";
import { cycleEnd, cycleStart, type WeeklyReset } from "@/lib/cycle";

/**
 * The stats' time range: a usage cycle or a calendar month (paged with `offset`, 0 = the current one, -1 the one
 * before…), the last N days up to now, or everything.
 */
export type RangeKind = "cycle" | "month" | "last7" | "last30" | "last90" | "all";
export interface RangeSel {
  kind: RangeKind;
  offset: number;
}

export const DEFAULT_RANGE: RangeSel = { kind: "cycle", offset: 0 };

/** The dropdown's choices, in groups. */
export const RANGE_MENU: { group: string | null; items: { label: string; sel: RangeSel }[] }[] = [
  {
    group: "周期",
    items: [
      { label: "本周期", sel: { kind: "cycle", offset: 0 } },
      { label: "上一周期", sel: { kind: "cycle", offset: -1 } },
    ],
  },
  {
    group: "月份",
    items: [
      { label: "本月", sel: { kind: "month", offset: 0 } },
      { label: "上月", sel: { kind: "month", offset: -1 } },
    ],
  },
  {
    group: "最近",
    items: [
      { label: "最近 7 天", sel: { kind: "last7", offset: 0 } },
      { label: "最近 30 天", sel: { kind: "last30", offset: 0 } },
      { label: "最近 90 天", sel: { kind: "last90", offset: 0 } },
    ],
  },
  { group: null, items: [{ label: "全部", sel: { kind: "all", offset: 0 } }] },
];

export const sameRange = (a: RangeSel, b: RangeSel) => a.kind === b.kind && a.offset === b.offset;

/** Cycles and months page with ‹ ›; the rolling ranges and 全部 do not. */
export const pageable = (k: RangeKind) => k === "cycle" || k === "month";

export interface ResolvedRange {
  /** What the stats query covers (`toMs` null: up to now). */
  timeRange: TimeRange | null;
  /** Its length in days (active days are counted out of it); null for 全部. */
  days: number | null;
  /** The dropdown's label. */
  label: string;
  /** Beside it: the exact dates, and what is left of the current cycle or month. */
  detail: string;
}

const DAY = 86_400_000;
const fmtReset = (d: Date) => format(d, "M月d日 EEE HH:mm", { locale: zhCN });
const fmtMonth = (d: Date) => format(d, "yyyy 年 M 月");

/** What is left until `end`: days and hours, or hours and minutes under a day. */
export function remaining(ms: number): string {
  const h = Math.max(0, Math.floor(ms / 3_600_000));
  return h >= 24 ? `${Math.floor(h / 24)} 天 ${h % 24} 小时` : `${h} 小时 ${Math.floor((ms % 3_600_000) / 60_000)} 分`;
}

export function resolveRange(sel: RangeSel, now: Date, reset: WeeklyReset): ResolvedRange {
  switch (sel.kind) {
    case "cycle": {
      const start = cycleStart(now, reset);
      start.setDate(start.getDate() + 7 * sel.offset);
      const end = cycleEnd(start);
      const current = sel.offset === 0;
      return {
        timeRange: { fromMs: start.getTime(), toMs: current ? null : end.getTime() },
        days: 7,
        label: current ? "本周期" : sel.offset === -1 ? "上一周期" : `${format(start, "M月d日")}起的周期`,
        detail: `${fmtReset(start)} 至 ${fmtReset(end)}${current ? `，还剩 ${remaining(end.getTime() - now.getTime())}` : ""}`,
      };
    }
    case "month": {
      const start = new Date(now.getFullYear(), now.getMonth() + sel.offset, 1);
      const end = new Date(start.getFullYear(), start.getMonth() + 1, 1);
      const current = sel.offset === 0;
      return {
        timeRange: { fromMs: start.getTime(), toMs: current ? null : end.getTime() },
        days: Math.round((end.getTime() - start.getTime()) / DAY),
        label: current ? "本月" : sel.offset === -1 ? "上月" : fmtMonth(start),
        detail: `${fmtMonth(start)}${current ? `，还剩 ${remaining(end.getTime() - now.getTime())}` : ""}`,
      };
    }
    case "last7":
    case "last30":
    case "last90": {
      const days = Number(sel.kind.slice(4));
      return {
        timeRange: { fromMs: now.getTime() - days * DAY, toMs: null },
        days,
        label: `最近 ${days} 天`,
        detail: "",
      };
    }
    case "all":
      return { timeRange: null, days: null, label: "全部", detail: "" };
  }
}
