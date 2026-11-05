import type { Stats } from "@/ipc/bindings";
import { formatCount } from "@/lib/format";

const DAY = 86_400_000;

/** The longest run of consecutive days (`YYYY-MM-DD`, any order, repeats allowed). */
export function longestStreak(days: string[]): number {
  const sorted = [...new Set(days)].sort();
  let best = 0;
  let run = 0;
  let prev = 0;
  for (const d of sorted) {
    const [y, m, dd] = d.split("-").map(Number);
    const t = Date.UTC(y, m - 1, dd);
    run = prev && t - prev === DAY ? run + 1 : 1;
    prev = t;
    best = Math.max(best, run);
  }
  return best;
}

/** Input tokens over the range: uncached input, cache reads and cache writes. */
export function inputTotals(daily: Stats["daily"]) {
  return daily.reduce(
    (a, d) => ({ input: a.input + d.input, read: a.read + d.cacheRead, write: a.write + d.cacheCreation }),
    { input: 0, read: 0, write: 0 },
  );
}

/** A daily average: one decimal below 10, so a quiet range does not read as 0. */
export const perDay = (v: number) => (v < 10 ? String(Math.round(v * 10) / 10) : formatCount(Math.round(v)));
