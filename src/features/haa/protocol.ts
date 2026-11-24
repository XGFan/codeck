/** Worker states reported by `haa ls --all --json --watch` (`haa/src/protocol.ts`). */
export type HaaWorkerState = "running" | "asking" | "idle" | "dormant" | "taken" | "dead";

const VALID_STATES: ReadonlySet<string> = new Set<HaaWorkerState>([
  "running",
  "asking",
  "idle",
  "dormant",
  "taken",
  "dead",
]);

function isHaaWorkerState(v: unknown): v is HaaWorkerState {
  return typeof v === "string" && VALID_STATES.has(v);
}

/** Subset of haa's `WorkerInfo` that CoDeck renders in the desktop status bar. */
export interface HaaWorker {
  id: string;
  owner: string;
  name: string;
  state: HaaWorkerState;
  tokens: number;
  ctx?: number;
  task?: string;
  title?: string;
  activity?: string;
  startedAt?: number;
  pending?: string;
}

const optStr = (v: unknown): string | undefined => (typeof v === "string" && v.length > 0 ? v : undefined);
const optNum = (v: unknown): number | undefined => (typeof v === "number" && Number.isFinite(v) ? v : undefined);

/**
 * Tolerantly parses one JSONL line from `haa ls --all --json --watch`.
 * Returns `null` if the line is not valid JSON or not a JSON array (caller drops bad lines instead of clearing state).
 * Malformed entries inside an array are skipped individually.
 */
export function parseHaaWorkers(raw: string): HaaWorker[] | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!Array.isArray(parsed)) return null;

  const out: HaaWorker[] = [];
  for (const item of parsed) {
    if (!item || typeof item !== "object") continue;
    const r = item as Record<string, unknown>;
    if (typeof r.owner !== "string" || typeof r.name !== "string" || !isHaaWorkerState(r.state)) continue;
    const id = typeof r.id === "string" && r.id.length > 0 ? r.id : `${r.owner}:${r.name}`;
    const tokens = typeof r.tokens === "number" && Number.isFinite(r.tokens) && r.tokens >= 0 ? r.tokens : 0;
    const w: HaaWorker = { id, owner: r.owner, name: r.name, state: r.state, tokens };
    const ctx = optNum(r.ctx);
    if (ctx !== undefined) w.ctx = ctx;
    const task = optStr(r.task);
    if (task !== undefined) w.task = task;
    const title = optStr(r.title);
    if (title !== undefined) w.title = title;
    const activity = optStr(r.activity);
    if (activity !== undefined) w.activity = activity;
    const startedAt = optNum(r.startedAt);
    if (startedAt !== undefined) w.startedAt = startedAt;
    const pending = optStr(r.pending);
    if (pending !== undefined) w.pending = pending;
    out.push(w);
  }
  return out;
}
