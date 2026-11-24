/**
 * 「已完成」与系统通知的规则（纯函数）：每个 Live Session 记着它现在的状态、从何时起、看过没有、通知发过没有。
 *
 * - 一轮结束（工作中 → 空闲）时没在看它，就标「已完成」；看了它、或它又工作 / 等待作答，就清掉。
 * - 等待作答或已完成持续到设定的时长、期间没看过它，发一次通知；状态变了重新计时。
 * - 「看」是窗口在前台、打开的就是它。
 */

/** What a Live Session is doing, as far as attention goes. */
export type Status = "busy" | "waiting" | "idle";

export interface Track {
  status: Status;
  /** When it got this status. */
  since: number;
  /** Its turn ended (busy → idle) while nobody was looking, and it has not been looked at since. */
  done: boolean;
  /** Looked at since `since`. */
  seen: boolean;
  /** Its notification for this status was sent. */
  notified: boolean;
}

export type Tracks = Record<string, Track>;

/** How long a Session waits (`waiting`) or stays done (`done`) unseen before a notification; null: none. */
export interface Delays {
  waiting: number | null;
  done: number | null;
}

export type DueKind = "waiting" | "done";

export interface Due {
  id: string;
  kind: DueKind;
  since: number;
}

/**
 * The tracks after the Live Sessions' statuses `now` at `at`, `viewing` the Session being looked at (null: none).
 * Sessions no longer Live are dropped. Returns `prev` itself when nothing changed.
 */
export function step(prev: Tracks, now: Record<string, Status>, at: number, viewing: string | null): Tracks {
  const next: Tracks = {};
  let changed = Object.keys(prev).length !== Object.keys(now).length;
  for (const [id, status] of Object.entries(now)) {
    const p = prev[id];
    const seen = viewing === id;
    let t: Track;
    if (p && p.status === status) t = seen && (!p.seen || p.done) ? { ...p, seen: true, done: false } : p;
    else t = { status, since: at, done: p?.status === "busy" && status === "idle" && !seen, seen, notified: false };
    if (t !== p) changed = true;
    next[id] = t;
  }
  return changed ? next : prev;
}

/** The notifications due at `at`, oldest first. */
export function due(tracks: Tracks, at: number, delays: Delays): Due[] {
  const out: Due[] = [];
  for (const [id, t] of Object.entries(tracks)) {
    if (t.notified || t.seen) continue;
    if (t.status === "waiting" && delays.waiting != null && at - t.since >= delays.waiting)
      out.push({ id, kind: "waiting", since: t.since });
    else if (t.status === "idle" && t.done && delays.done != null && at - t.since >= delays.done)
      out.push({ id, kind: "done", since: t.since });
  }
  return out.sort((a, b) => a.since - b.since || a.id.localeCompare(b.id));
}

/** `tracks` with these Sessions' notifications sent. */
export function markNotified(tracks: Tracks, ids: string[]): Tracks {
  const next = { ...tracks };
  for (const id of ids) if (next[id]) next[id] = { ...next[id], notified: true };
  return next;
}

/** What a notification shows about one Session. */
export interface Subject extends Due {
  title: string;
  /** Its Project's name; null when unknown. */
  project: string | null;
}

/**
 * One notification for everything due at once: a single Session by its title, several counted. A click opens the
 * Session it names (the first waiting one, else the first done).
 */
export function message(items: Subject[], at: number): { title: string; body: string; sessionId: string } {
  const first = items.find((i) => i.kind === "waiting") ?? items[0];
  if (items.length === 1) {
    const mins = Math.floor((at - first.since) / 60_000);
    const what = first.kind === "waiting" ? (mins > 0 ? `等待作答 ${mins} 分钟` : "等待作答") : "已完成";
    return { title: first.title, body: first.project ? `${what} · ${first.project}` : what, sessionId: first.id };
  }
  const waiting = items.filter((i) => i.kind === "waiting").length;
  const done = items.length - waiting;
  const body = [waiting > 0 && `${waiting} 个等待作答`, done > 0 && `${done} 个已完成`].filter(Boolean).join("，");
  return { title: `${items.length} 个 Session 需要处理`, body, sessionId: first.id };
}
