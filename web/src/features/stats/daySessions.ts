import type { SessionUsage, TimeSpan } from "@/ipc/bindings";

export const MAX_DAY_SESSIONS = 6;

export interface SpanSegment {
  leftPct: number;
  widthPct: number;
}

export interface DaySessionRow {
  sessionId: string;
  title: string;
  cost: number;
  requests: number;
  segments: SpanSegment[];
}

export interface DaySessionsView {
  total: number;
  subtitle: string;
  rows: DaySessionRow[];
  restCount: number;
  restCost: number;
}

/** Converts activity spans into `[0, 100]` percentage segments along `[dayStartMs, dayEndMs)`. */
export function buildSpanSegments(spans: TimeSpan[], dayStartMs: number, dayEndMs: number): SpanSegment[] {
  const duration = Math.max(1, dayEndMs - dayStartMs);
  const out: SpanSegment[] = [];
  for (const sp of spans) {
    if (sp.endMs < dayStartMs || sp.startMs >= dayEndMs) continue;
    const s = Math.max(dayStartMs, Math.min(dayEndMs, sp.startMs));
    const e = Math.max(s, Math.min(dayEndMs, sp.endMs));
    const leftPct = ((s - dayStartMs) / duration) * 100;
    const widthPct = ((e - s) / duration) * 100;
    out.push({ leftPct, widthPct });
  }
  return out;
}

/**
 * Splits a day's sessions into top 6 by Cost (with normalized timeline segments) plus a summary row for the rest.
 */
export function buildDaySessionsView(sessions: SessionUsage[], dayStartMs: number, dayEndMs: number): DaySessionsView {
  const sorted = [...sessions].sort(
    (a, b) => b.cost - a.cost || b.requests - a.requests || a.sessionId.localeCompare(b.sessionId),
  );
  const total = sorted.length;
  const top = sorted.slice(0, MAX_DAY_SESSIONS);
  const rest = sorted.slice(MAX_DAY_SESSIONS);
  const restCount = rest.length;
  const restCost = rest.reduce((acc, s) => acc + s.cost, 0);
  const subtitle =
    total === 0
      ? ""
      : total <= MAX_DAY_SESSIONS
        ? `这一天有 ${total} 个`
        : `这一天有 ${total} 个，按 Cost 列前 ${MAX_DAY_SESSIONS} 个`;

  return {
    total,
    subtitle,
    rows: top.map((s) => ({
      sessionId: s.sessionId,
      title: s.title,
      cost: s.cost,
      requests: s.requests,
      segments: buildSpanSegments(s.spans, dayStartMs, dayEndMs),
    })),
    restCount,
    restCost,
  };
}
