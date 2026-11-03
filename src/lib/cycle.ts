/** Weekly usage cycle: resets every week at `weekday` (0 = Monday) and `minutes` past local midnight. */
export interface WeeklyReset {
  weekday: number;
  minutes: number;
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
