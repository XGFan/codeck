import type { NotifySettings } from "@/ipc/bindings";

/** Where an earlier version kept the notification delays (ms; null = off). */
export const LEGACY_KEY = "codeck.notify";

/** The delays an earlier version saved (`raw`, its localStorage value) as the Host's settings take them; null: none. */
export function legacyDelays(
  raw: string | null,
): Partial<Pick<NotifySettings, "waitingMinutes" | "doneMinutes">> | null {
  if (raw == null) return null;
  let saved: unknown;
  try {
    saved = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof saved !== "object" || saved == null) return null;
  const minutes = (v: unknown) =>
    v === null ? null : typeof v === "number" && v >= 0 ? Math.round(v / 60_000) : undefined;
  const out: Partial<Pick<NotifySettings, "waitingMinutes" | "doneMinutes">> = {};
  const { waiting, done } = saved as Record<string, unknown>;
  const w = minutes(waiting);
  const d = minutes(done);
  if (w !== undefined) out.waitingMinutes = w;
  if (d !== undefined) out.doneMinutes = d;
  return out;
}
