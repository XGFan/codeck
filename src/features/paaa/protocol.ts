/** Worker states reported by `paaa ls --all --json --watch` (`PaaA/src/protocol.ts`). */
export type PaaaWorkerState = "running" | "asking" | "idle" | "dormant" | "taken" | "dead";

const VALID_STATES: ReadonlySet<string> = new Set<PaaaWorkerState>([
  "running",
  "asking",
  "idle",
  "dormant",
  "taken",
  "dead",
]);

function isPaaaWorkerState(v: unknown): v is PaaaWorkerState {
  return typeof v === "string" && VALID_STATES.has(v);
}

/** Subset of paaa's `WorkerInfo` that CoDeck renders in the desktop status bar. */
export interface PaaaWorker {
  id: string;
  owner: string;
  name: string;
  state: PaaaWorkerState;
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
 * Tolerantly parses one JSONL line from `paaa ls --all --json --watch`.
 * Returns `null` if the line is not valid JSON or not a JSON array (caller drops bad lines instead of clearing state).
 * Malformed entries inside an array are skipped individually.
 */
export function parsePaaaWorkers(raw: string): PaaaWorker[] | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!Array.isArray(parsed)) return null;

  const out: PaaaWorker[] = [];
  for (const item of parsed) {
    if (!item || typeof item !== "object") continue;
    const r = item as Record<string, unknown>;
    if (typeof r.owner !== "string" || typeof r.name !== "string" || !isPaaaWorkerState(r.state)) continue;
    const id = typeof r.id === "string" && r.id.length > 0 ? r.id : `${r.owner}:${r.name}`;
    const tokens = typeof r.tokens === "number" && Number.isFinite(r.tokens) && r.tokens >= 0 ? r.tokens : 0;
    const w: PaaaWorker = { id, owner: r.owner, name: r.name, state: r.state, tokens };
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
