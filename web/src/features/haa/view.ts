import { formatDuration } from "@/lib/format";
import type { HaaWorker, HaaWorkerState } from "@/ipc/protocol/haa";

export const STATE_LABEL: Record<HaaWorkerState, string> = {
  running: "运行中",
  asking: "等待中",
  taken: "接管中",
  idle: "空闲",
  dormant: "休眠",
  dead: "已退出",
};

const STATE_RANK: Record<HaaWorkerState, number> = {
  running: 0,
  asking: 1,
  taken: 2,
  idle: 3,
  dormant: 4,
  dead: 5,
};

const SUMMARY_STATES: readonly HaaWorkerState[] = ["idle", "dormant", "dead"];

const MINUTE_MS = 60_000;
const HOUR_MS = 3_600_000;
const DAY_MS = 86_400_000;

export function isActiveState(state: HaaWorkerState): boolean {
  return state === "running" || state === "asking" || state === "taken";
}

export function isActiveWorker(w: HaaWorker): boolean {
  return isActiveState(w.state);
}

/** Elapsed duration in ms: `now - startedAt` for running/asking, `now - lastActive` otherwise; `null` if missing. */
export function workerDurationMs(w: HaaWorker, now: number): number | null {
  const ts = w.state === "running" || w.state === "asking" ? w.startedAt : w.lastActive;
  if (ts == null) return null;
  return Math.max(0, now - ts);
}

/** Formats duration text for the table cell. */
export function formatWorkerDuration(w: HaaWorker, now: number): string {
  const ms = workerDurationMs(w, now);
  if (ms == null) return "—";
  if (w.state === "running" || w.state === "asking") {
    if (ms < 1000) return "0 秒";
    return formatDuration(ms, "detail");
  }
  if (ms < MINUTE_MS) return "刚刚";
  if (ms < HOUR_MS) return `${Math.floor(ms / MINUTE_MS)} 分钟`;
  if (ms < DAY_MS) return `${Math.floor(ms / HOUR_MS)} 小时`;
  return `${Math.floor(ms / DAY_MS)} 天`;
}

/** Sorts workers by state priority, then by duration ascending (missing timestamps last). */
export function sortWorkers(workers: readonly HaaWorker[], now: number): HaaWorker[] {
  return [...workers].sort((a, b) => {
    const rankDiff = STATE_RANK[a.state] - STATE_RANK[b.state];
    if (rankDiff !== 0) return rankDiff;
    const da = workerDurationMs(a, now) ?? Infinity;
    const db = workerDurationMs(b, now) ?? Infinity;
    if (da === db) return 0;
    return da - db;
  });
}

/** The backend of a haa model name (`<backend>/<id>[:thinking]`); null for a name without one (older haa). */
export function workerBackend(model: string | null | undefined): string | null {
  const i = model ? model.indexOf("/") : -1;
  return i > 0 ? model!.slice(0, i) : null;
}

/**
 * A codex or pi Worker: CoDeck reads its conversation from the agent's own file by Session id, so it opens although it
 * is not in the Session list (an agy Worker's opens once its Session is listed).
 */
export function readsWorkerById(model: string | null | undefined): boolean {
  return isReadById(workerBackend(model));
}

/** A backend whose Workers' conversations CoDeck reads by Session id from the backend's own records. */
export function isReadById(backend: string | null | undefined): boolean {
  return backend === "codex" || backend === "pi";
}

/** Returns the part after the last `/` in a model identifier. */
export function shortModelName(model: string | undefined): string {
  if (!model) return "";
  const i = model.lastIndexOf("/");
  return i >= 0 ? model.slice(i + 1) : model;
}

export interface TaskCell {
  prefix?: string;
  text: string;
  muted: boolean;
}

/** Structured display content for the Task column. */
export function formatTaskCell(w: HaaWorker): TaskCell {
  if (w.state === "asking" && w.pending) {
    const collapsed = w.pending.trim().replace(/\s+/g, " ");
    if (collapsed) {
      return { prefix: "问 Main：", text: collapsed, muted: false };
    }
  }
  if (isActiveState(w.state)) {
    return { text: w.title ?? w.task ?? "", muted: false };
  }
  return {
    text: w.tasks != null ? `做过 ${w.tasks} 个 Task` : "",
    muted: true,
  };
}

/** Plain text for the Task column. */
export function formatTaskText(w: HaaWorker): string {
  const cell = formatTaskCell(w);
  return cell.prefix ? `${cell.prefix}${cell.text}` : cell.text;
}

/** Native `title` tooltip for the Task column. */
export function formatTaskTooltip(w: HaaWorker): string {
  if (w.state === "asking" && w.pending) {
    return `问 Main：${w.pending}`;
  }
  if (w.state === "running") {
    const base = w.title ?? w.task ?? "";
    if (w.activity) {
      return base ? `${base}\n正在做：${w.activity}` : `正在做：${w.activity}`;
    }
    return base;
  }
  return formatTaskText(w);
}

/** Collapsed summary for inactive states, e.g. `3 个空闲 · 4 个休眠`. */
export function formatCollapsedSummary(workers: readonly HaaWorker[]): string {
  const parts: string[] = [];
  for (const state of SUMMARY_STATES) {
    const n = workers.filter((w) => w.state === state).length;
    if (n > 0) parts.push(`${n} 个${STATE_LABEL[state]}`);
  }
  return parts.join(" · ");
}
