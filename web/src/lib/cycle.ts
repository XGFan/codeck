/** Weekly usage cycle: resets every week at `weekday` (0 = Monday) and `minutes` past local midnight. */
export interface WeeklyReset {
  weekday: number;
  minutes: number;
}

export const WEEK_MS = 7 * 86_400_000;

/**
 * How a usage cycle's boundaries are determined:
 * - `anchor`: every 7×24h absolute interval anchored at `anchorMs` (subscription's weekly reset, rounded to whole minutes);
 * - `weekly`: local wall-clock weekday & minute from Settings (fallback when no subscription quota is available).
 */
export type CycleRule = { kind: "anchor"; anchorMs: number } | { kind: "weekly"; reset: WeeklyReset };

/** Rounds a unix-epoch millisecond timestamp to the nearest whole minute. */
export function roundToMinute(ms: number): Date {
  return new Date(Math.round(ms / 60_000) * 60_000);
}

/** Start of the cycle containing `now`: the latest reset at or before it (local wall-clock time). */
export function cycleStart(now: Date, reset: WeeklyReset): Date {
  const d = new Date(now);
  const daysBack = (((d.getDay() + 6) % 7) - reset.weekday + 7) % 7;
  d.setDate(d.getDate() - daysBack);
  d.setHours(Math.floor(reset.minutes / 60), reset.minutes % 60, 0, 0);
  if (d > now) d.setDate(d.getDate() - 7);
  return d;
}

/** The reset ending the cycle that starts at `start`. */
export function cycleEnd(start: Date): Date {
  const d = new Date(start);
  d.setDate(d.getDate() + 7);
  return d;
}

/**
 * Start and end `[start, end)` of the cycle containing `now`, stepped by `offset` cycles (0 = current, -1 = previous…).
 * For `anchor`, steps in exact 7×24h intervals from `anchorMs` without local wall-clock recomputation.
 */
export function cycleBounds(now: Date, rule: CycleRule, offset = 0): { start: Date; end: Date } {
  if (rule.kind === "anchor") {
    const startMs = rule.anchorMs - Math.ceil((rule.anchorMs - now.getTime()) / WEEK_MS) * WEEK_MS + offset * WEEK_MS;
    return { start: new Date(startMs), end: new Date(startMs + WEEK_MS) };
  }
  const start = cycleStart(now, rule.reset);
  if (offset !== 0) start.setDate(start.getDate() + 7 * offset);
  return { start, end: cycleEnd(start) };
}
