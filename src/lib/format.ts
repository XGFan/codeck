import { differenceInCalendarDays, differenceInMinutes, format } from "date-fns";
import { zhCN } from "date-fns/locale";

const TOKEN_UNITS: [number, string, number][] = [
  [1e3, "K", 1],
  [1e6, "M", 2],
  [1e9, "B", 2],
];

/** 48300 -> "48.3K"，1210000 -> "1.21M"，1543000000 -> "1.54B"；进位到 1000 时换下一个单位（999999 -> "1M"）。 */
export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  for (const [i, [size, unit, digits]] of TOKEN_UNITS.entries()) {
    const v = Number((n / size).toFixed(digits));
    if (v < 1000 || i === TOKEN_UNITS.length - 1) return `${trim(v, digits)}${unit}`;
  }
  return String(n);
}

const counts = new Intl.NumberFormat("en-US");

/** 计数，千位加逗号：1234567 -> "1,234,567"。 */
export const formatCount = (n: number): string => counts.format(n);

/** 美元金额：0 -> "$0"，0.004 -> "<$0.01"，12.345 -> "$12.35"，1234.5 -> "$1,235"。 */
export function formatCost(usd: number): string {
  if (usd <= 0) return "$0";
  if (usd < 0.01) return "<$0.01";
  if (usd < 1000) return `$${usd.toFixed(2)}`;
  return `$${formatCount(Math.round(usd))}`;
}

function trim(v: number, digits: number): string {
  return v.toFixed(digits).replace(/\.?0+$/, "");
}

/** 毫秒 -> "1 小时 12 分"、"5 分"、"30 秒"。 */
/**
 * 时长，两档精度（docs/spec.md「界面」）：
 * - `overview`（Session 时长、Connect 的用时）到分钟：「1 小时 12 分」「5 分」「30 秒」；
 * - `detail`（每轮「已处理」、Subagent、工具调用）到秒：「1 小时 12 分」「1 分 04 秒」「30 秒」，不足 1 秒写「320 毫秒」。
 * 没有值或为负时返回空串。
 */
export function formatDuration(ms: number | null | undefined, precision: "overview" | "detail" = "overview"): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return "";
  if (precision === "detail" && Math.round(ms) < 1000) return `${Math.round(ms)} 毫秒`;
  const totalSec = Math.round(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  if (h > 0) return m > 0 ? `${h} 小时 ${m} 分` : `${h} 小时`;
  if (m > 0) return precision === "detail" ? `${m} 分 ${String(s).padStart(2, "0")} 秒` : `${m} 分`;
  return `${totalSec} 秒`;
}

/** 刚刚 / N 分钟前 / N 小时前 / 昨天 / 周X（一周内）/ M月D日。 */
export function formatRelative(date: Date | number, now: Date | number = new Date()): string {
  const mins = differenceInMinutes(now, date);
  if (mins < 1) return "刚刚";
  if (mins < 60) return `${mins} 分钟前`;
  const days = differenceInCalendarDays(now, date);
  if (days === 0) return `${Math.floor(mins / 60)} 小时前`;
  if (days === 1) return "昨天";
  if (days < 7) return format(date, "EEEE", { locale: zhCN }).replace("星期", "周");
  return format(date, "M月d日", { locale: zhCN });
}

/** A time span `[fromMs, toMs)`: `10月22日 04:00–05:00` within a day, `10月22日` for a whole day, `10月20日–10月26日` across days. */
export function formatSpan(fromMs: number, toMs: number): string {
  const s = new Date(fromMs);
  const last = new Date(toMs - 1);
  const end = new Date(toMs);
  const md = (d: Date) => `${d.getMonth() + 1}月${d.getDate()}日`;
  const hm = (d: Date) => `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  if (md(s) !== md(last)) return `${md(s)}–${md(last)}`;
  if (s.getHours() === 0 && s.getMinutes() === 0 && end.getHours() === 0 && end.getMinutes() === 0) return md(s);
  return `${md(s)} ${hm(s)}–${end.getHours() === 0 && end.getMinutes() === 0 ? "24:00" : hm(end)}`;
}
