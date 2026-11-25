import { format } from "date-fns";
import { zhCN } from "date-fns/locale";
import type { PlanLimit, PlanLimitKind, PlanUsage } from "@/ipc/bindings";
import { roundToMinute, WEEK_MS, type CycleRule, type WeeklyReset } from "@/lib/cycle";
import { errorText } from "@/lib/errors";

export const PLAN_CYCLE_ANCHOR_KEY = "codeck.planCycleAnchor";
export const MIN_ANCHOR_MS = Date.UTC(2020, 0, 1);
export const MAX_ANCHOR_MS = Date.UTC(2100, 0, 1);

const FIVE_HOURS_MS = 5 * 3_600_000;
const AHEAD_THRESHOLD_PCT = 3;

export type PlanState =
  | { kind: "loading" }
  | { kind: "ok"; usage: PlanUsage; error: string | null }
  | { kind: "none" }
  | { kind: "failed"; error: string };

export type QuotaTone = "normal" | "warn" | "full";

export interface MeterView {
  kind: PlanLimitKind;
  label: string;
  percent: number;
  fillPct: number;
  tone: QuotaTone;
  /** True when clamped usage exceeds unrounded elapsed time by at least 3 percentage points. */
  isAhead: boolean;
  /** Unrounded elapsed percentage (0–100) where the deepened segment starts; null when not ahead. */
  aheadStartPct: number | null;
  /** Rounded percentage points ahead of elapsed time; null when not ahead. */
  aheadByPct: number | null;
  /** Hover title on the progress bar (`时间已过 N%`), or null when reset is absent/past. */
  elapsedTitle: string | null;
  /** Status and reset caption (`[快用完了 · ]X 后重置[ · 今天 16:40]`). */
  subtext: string;
  /** Strip-only ahead caption (`超前 N%`), rendered beside a colored swatch. */
  aheadText: string | null;
  ariaLabel: string;
}

export interface RingView {
  percent: number;
  tone: QuotaTone;
  ariaLabel: string;
}

/**
 * Derives the 4-state UI model from a TanStack Query result:
 * - `loading`: no successful result yet and no error;
 * - `ok`: last success was `Some` (if a later refetch failed, keeps the last data with `error`);
 * - `none`: last success was `null`;
 * - `failed`: never succeeded and has an error.
 */
export function derivePlanState(input: { data: PlanUsage | null | undefined; error: unknown }): PlanState {
  if (input.data !== undefined) {
    if (input.data === null) return { kind: "none" };
    return { kind: "ok", usage: input.data, error: input.error != null ? errorText(input.error) : null };
  }
  if (input.error != null) return { kind: "failed", error: errorText(input.error) };
  return { kind: "loading" };
}

/** Formats the subscription plan badge (`max` → `Max`); null when absent/blank. */
export function formatPlanBadge(plan: string | null): string | null {
  const s = plan?.trim();
  if (!s) return null;
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** Display label of one rate-limit bucket. */
export function limitLabel(limit: PlanLimit): string {
  switch (limit.kind) {
    case "session":
      return "当前 5 小时";
    case "weeklyAll":
      return "本周 · 全部模型";
    case "weeklyScoped": {
      const m = limit.model?.trim();
      return m ? `本周 · ${m}` : "本周 · 模型";
    }
  }
}

/** Threshold tone judged on raw percent (`>= 80` warn, `>= 100` full); display percent is rounded separately. */
export function quotaTone(rawPercent: number): QuotaTone {
  if (rawPercent >= 100) return "full";
  if (rawPercent >= 80) return "warn";
  return "normal";
}

/** Countdown until reset (`4 天 21 小时`, `2 小时 5 分`, `48 分`). Expects `ms > 0`. */
export function formatResetCountdown(ms: number): string {
  const totalMin = Math.max(1, Math.round(ms / 60_000));
  if (totalMin >= 24 * 60) {
    const totalHours = Math.floor(totalMin / 60);
    const d = Math.floor(totalHours / 24);
    const h = totalHours % 24;
    return h > 0 ? `${d} 天 ${h} 小时` : `${d} 天`;
  }
  if (totalMin >= 60) {
    const h = Math.floor(totalMin / 60);
    const m = totalMin % 60;
    return m > 0 ? `${h} 小时 ${m} 分` : `${h} 小时`;
  }
  return `${totalMin} 分`;
}

/** Reset wall-clock moment rounded to the nearest minute: `今天 16:40` or `11月30日 周一 14:00`. */
export function formatResetAt(resetsMs: number, nowMs: number): string {
  const d = roundToMinute(resetsMs);
  const today = new Date(nowMs);
  const sameDay =
    d.getFullYear() === today.getFullYear() && d.getMonth() === today.getMonth() && d.getDate() === today.getDate();
  if (sameDay) return `今天 ${format(d, "HH:mm")}`;
  return format(d, "M月d日 EEE HH:mm", { locale: zhCN });
}

/** Footer timestamp: `HH:MM 更新`, or `读取失败，显示的是 HH:MM 的数据` when stale after a failed refresh. */
export function formatUpdatedAt(fetchedMs: number, error: string | null): string {
  const hm = format(new Date(fetchedMs), "HH:mm");
  return error != null ? `读取失败，显示的是 ${hm} 的数据` : `${hm} 更新`;
}

/** Builds all display fields for one rate-limit meter in either the top strip or the sidebar popover. */
export function buildMeterView(limit: PlanLimit, nowMs: number, variant: "strip" | "popover"): MeterView {
  const label = limitLabel(limit);
  const raw = Number.isFinite(limit.percent) ? Math.max(0, limit.percent) : 0;
  const percent = Math.round(raw);
  const fillPct = Math.min(100, raw);
  const tone = quotaTone(raw);
  const statusText = tone === "full" ? "已用完" : tone === "warn" ? "快用完了" : null;

  const hasReset = limit.resetsMs != null && Number.isFinite(limit.resetsMs) && limit.resetsMs > nowMs;
  let isAhead = false;
  let aheadStartPct: number | null = null;
  let aheadByPct: number | null = null;
  let elapsedTitle: string | null = null;
  let subtext = statusText ?? "";
  let aheadText: string | null = null;

  if (hasReset) {
    const resetsMs = limit.resetsMs!;
    const windowMs = limit.kind === "session" ? FIVE_HOURS_MS : WEEK_MS;
    const startMs = resetsMs - windowMs;
    const rawElapsed = Math.min(100, Math.max(0, ((nowMs - startMs) * 100) / windowMs));
    elapsedTitle = `时间已过 ${Math.round(rawElapsed)}%`;

    const diff = fillPct - rawElapsed;
    if (diff >= AHEAD_THRESHOLD_PCT) {
      isAhead = true;
      aheadStartPct = rawElapsed;
      aheadByPct = Math.round(diff);
    }

    const resetIn = `${formatResetCountdown(resetsMs - nowMs)}后重置`;
    if (variant === "strip") {
      subtext = statusText ? `${statusText} · ${resetIn}` : resetIn;
      if (isAhead && aheadByPct != null) aheadText = `超前 ${aheadByPct}%`;
    } else {
      const resetAt = formatResetAt(resetsMs, nowMs);
      subtext = statusText ? `${statusText} · ${resetIn} · ${resetAt}` : `${resetIn} · ${resetAt}`;
    }
  }

  const ariaLabel =
    isAhead && aheadByPct != null
      ? `${label}已用 ${percent}%，比时间进度多 ${aheadByPct}%`
      : `${label}已用 ${percent}%`;

  return {
    kind: limit.kind,
    label,
    percent,
    fillPct,
    tone,
    isAhead,
    aheadStartPct,
    aheadByPct,
    elapsedTitle,
    subtext,
    aheadText,
    ariaLabel,
  };
}

/** What the sidebar's quota button says and how it is toned: the highest used limit. */
export function buildRingView(usage: PlanUsage): RingView {
  const maxRaw = usage.limits.reduce((max, l) => (Number.isFinite(l.percent) && l.percent > max ? l.percent : max), 0);
  const percent = Math.max(0, Math.round(maxRaw));
  const tone = quotaTone(maxRaw);
  return {
    percent,
    tone,
    ariaLabel: `额度：最高一项已用 ${percent}%`,
  };
}

/** Parses and validates a cycle anchor timestamp (finite, within 2020-01-01..2100-01-01), rounded to minute; else null. */
export function sanitizeAnchorMs(value: unknown): number | null {
  const n = typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" ? Number(value) : NaN;
  if (!Number.isFinite(n) || n < MIN_ANCHOR_MS || n > MAX_ANCHOR_MS) return null;
  const rounded = roundToMinute(n).getTime();
  return rounded >= MIN_ANCHOR_MS && rounded <= MAX_ANCHOR_MS ? rounded : null;
}

/** Returns `weeklyAll.resetsMs` validated and rounded to the nearest minute, or `null` when absent/invalid. */
export function weeklyAllResetMs(usage: PlanUsage): number | null {
  const item = usage.limits.find((l) => l.kind === "weeklyAll");
  return sanitizeAnchorMs(item?.resetsMs);
}

/**
 * Resolves the cycle anchor and what should be stored in `localStorage` (`codeck.planCycleAnchor`):
 * - `ok` with valid `weeklyAll.resetsMs`: use and persist it;
 * - `ok` without valid `weeklyAll.resetsMs`: keep saved untouched (if valid);
 * - `none`: clear saved and fall back to Settings;
 * - `loading` / `failed`: keep saved (if valid) and use it.
 */
export function resolvePlanAnchor(
  state: PlanState,
  savedAnchorMs: number | null,
): { nextSaved: number | null; anchorMs: number | null } {
  const validSaved = sanitizeAnchorMs(savedAnchorMs);
  switch (state.kind) {
    case "ok": {
      const ms = weeklyAllResetMs(state.usage);
      return ms != null ? { nextSaved: ms, anchorMs: ms } : { nextSaved: validSaved, anchorMs: validSaved };
    }
    case "none":
      return { nextSaved: null, anchorMs: null };
    case "loading":
    case "failed":
      return { nextSaved: validSaved, anchorMs: validSaved };
  }
}

export function cycleRuleFrom(anchorMs: number | null, weeklyReset: WeeklyReset): CycleRule {
  return anchorMs != null ? { kind: "anchor", anchorMs } : { kind: "weekly", reset: weeklyReset };
}

/** Formats an anchor timestamp as local weekday + HH:mm (rounded to minute) for Settings. */
export function formatAnchorReset(anchorMs: number): string {
  return format(roundToMinute(anchorMs), "EEE HH:mm", { locale: zhCN });
}
