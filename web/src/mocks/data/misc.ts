import type {
  AppInfo,
  DailyModelTokens,
  Diagnostics,
  HourModelUsage,
  HourUsage,
  IndexStatus,
  SessionUsage,
  SlashCommand,
  Stats,
  StatsRequest,
  TimeSpan,
} from "@/ipc/bindings";
import { costOf, DAY, HOUR, NOW, tok } from "./builders";
import { sessionData } from "./sessions";

export const appInfo = {
  version: "0.1.0",
  dataRoot: "/Users/dev/.claude",
  dataRootSource: "default",
  dataRootExists: true,
  cacheDir: "/Users/dev/Library/Caches/dev.joy.codeck",
  schemaVersion: 1,
  agyRoot: "/Users/dev/.gemini/antigravity-cli",
  agyRootExists: true,
} satisfies AppInfo;

export const indexStatus = {
  phase: "idle",
  filesTotal: 1342,
  filesDone: 1342,
  bytesTotal: 3_221_225_472,
  bytesDone: 3_221_225_472,
  sessionsTotal: 10,
  textReady: true,
  error: null,
} satisfies IndexStatus;

export const diagnostics = {
  filesScanned: 1342,
  sessions: 10,
  emptySessions: 2,
  failedLines: 3,
  filesWithFailures: [
    {
      path: "/Users/dev/.claude/projects/-Users-dev-Developer-orbit-web/5e1c0b7a-0006-4000-8000-000000000006.jsonl",
      failedLines: 3,
      firstError: "EOF while parsing a string at line 412 column 18",
    },
  ],
  unknownEntryTypes: [{ name: "frame-link", count: 14, versions: ["2.1.88", "2.1.91"] }],
  unknownBlockTypes: [{ name: "server_tool_use", count: 5, versions: ["2.1.91"] }],
  unknownSystemSubtypes: [{ name: "memory_saved", count: 2, versions: ["2.1.91"] }],
  unknownTools: [{ name: "ScheduleWakeup", count: 9, versions: ["2.1.90", "2.1.91"] }],
  unrecognizedMessages: [{ name: "<handoff-message>", count: 4, versions: ["2.1.91"] }],
  versions: [
    { version: "2.1.91", sessions: 6, failedLines: 0, unknownItems: 12, lastSeenMs: NOW - 120_000 },
    { version: "2.1.88", sessions: 3, failedLines: 3, unknownItems: 9, lastSeenMs: NOW - 5 * DAY },
    { version: "2.1.76", sessions: 1, failedLines: 0, unknownItems: 0, lastSeenMs: NOW - 20 * DAY },
  ],
  duplicateUuids: 2,
  orphanSubagents: 1,
} satisfies Diagnostics;

function localDay(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

const MODELS = ["claude-fable-5-1", "claude-opus-4-7", "claude-sonnet-5-5", "claude-haiku-4-5"] as const;
const MODEL_SHARES = [0.42, 0.36, 0.14, 0.08] as const;

/** Deterministic pseudo-random in [0, 1) keyed by integer seed. */
function hash(n: number): number {
  const x = Math.sin(n * 12.9898 + 78.233) * 43758.5453;
  return x - Math.floor(x);
}

/** Deterministic per-hour usage keyed purely by the absolute local hour (`hourStartMs`); empty after `cutoffMs`. */
function hourBreakdown(hourStartMs: number, cutoffMs: number): DailyModelTokens[] {
  if (hourStartMs > cutoffMs) return [];
  const d = new Date(hourStartMs);
  const dayKey = localDay(hourStartMs);
  const daySeed = d.getFullYear() * 10000 + (d.getMonth() + 1) * 100 + d.getDate();
  const h = d.getHours();
  // Quiet days (~10%, never the last 3 days before cutoffMs so drills on recent days always find activity).
  const ageDays = Math.floor((cutoffMs - hourStartMs) / DAY);
  if (ageDays > 3 && hash(daySeed) < 0.1) return [];
  // Sleeping hours mostly idle, daytime/evening active.
  const awake = h >= 9 && h <= 23 ? 0.68 : h <= 2 ? 0.3 : 0.05;
  const r = hash(daySeed * 31 + h * 7);
  if (r > awake && !(ageDays === 0 && h === new Date(cutoffMs).getHours())) return [];

  const totalReqs = Math.max(1, Math.round(hash(daySeed * 37 + h * 13) * 120) + 8);
  const rows: DailyModelTokens[] = [];
  let remainingReqs = totalReqs;
  for (let m = 0; m < MODELS.length; m++) {
    const model = MODELS[m]!;
    const reqs = m === MODELS.length - 1 ? remainingReqs : Math.max(1, Math.round(totalReqs * MODEL_SHARES[m]!));
    remainingReqs = Math.max(0, remainingReqs - reqs);
    if (reqs <= 0) continue;
    const k = daySeed * 100 + h * 5 + m;
    const scale = [1.2, 1.0, 0.5, 0.25][m]!;
    const input = Math.round((hash(k + 1) * 800 + 150) * reqs * scale);
    const output = Math.round((hash(k + 2) * 320 + 60) * reqs * scale);
    const cacheRead = Math.round((hash(k + 3) * 35_000 + 8_000) * reqs * scale);
    const cacheCreation = Math.round((hash(k + 4) * 2_200 + 300) * reqs * scale);
    const cost = costOf(model, tok(input, output, cacheRead, cacheCreation)) ?? 0;
    rows.push({ day: dayKey, model, input, output, cacheRead, cacheCreation, cost, requests: reqs });
  }
  return rows;
}

const ALL_HISTORY_DAYS = 400;
const DAY_DETAIL_MAX_MS = 26 * HOUR;
const SESSION_WEIGHTS = [0.3, 0.19, 0.14, 0.11, 0.08, 0.06, 0.05, 0.04, 0.03] as const;

/** Deterministic per-session cost, requests, and active spans for a single-day mock query, using existing mock sessions. */
function buildMockDaySessions(
  hourly: HourUsage[],
  totalCost: number,
  totalReqs: number,
  from: number,
  to: number,
  nowMs: number,
): SessionUsage[] {
  const activeHours = hourly.filter((h) => h.requests > 0 && h.startMs <= nowMs);
  if (activeHours.length === 0) return [];

  const pool = sessionData.filter((s) => s.summary.titleSource !== "untitled");
  const d = new Date(from);
  const daySeed = d.getFullYear() * 10000 + (d.getMonth() + 1) * 100 + d.getDate();
  const count = Math.min(pool.length, SESSION_WEIGHTS.length);
  const startIdx = daySeed % pool.length;
  const picked = Array.from({ length: count }, (_, i) => pool[(startIdx + i) % pool.length]!.summary);

  const rawWeights = SESSION_WEIGHTS.slice(0, count).map((w, i) => w * (0.85 + 0.3 * hash(daySeed * 17 + i)));
  const weightSum = rawWeights.reduce((a, b) => a + b, 0);
  const weights = rawWeights.map((w) => w / weightSum);

  let remCost = totalCost;
  let remReqs = totalReqs;
  const sessions: SessionUsage[] = picked.map((s, i) => {
    const isLast = i === count - 1;
    const cost = isLast ? Math.max(0, remCost) : totalCost * weights[i]!;
    remCost -= cost;
    const minLeft = count - 1 - i;
    const requests = isLast
      ? Math.max(0, remReqs)
      : totalReqs >= count
        ? Math.max(1, Math.min(remReqs - minLeft, Math.round(totalReqs * weights[i]!)))
        : Math.min(remReqs, Math.round(totalReqs * weights[i]!));
    remReqs = Math.max(0, remReqs - requests);

    const spanCount = Math.min(activeHours.length, i < 3 ? 4 : i < 6 ? 2 : 1);
    const step = Math.max(1, Math.floor(activeHours.length / spanCount));
    const offset = Math.floor(hash(daySeed * 41 + i * 11) * activeHours.length);
    const chosenIndices = new Set<number>();
    const spans: TimeSpan[] = [];
    for (let j = 0; j < spanCount; j++) {
      let hi = (offset + j * step) % activeHours.length;
      while (chosenIndices.has(hi)) hi = (hi + 1) % activeHours.length;
      chosenIndices.add(hi);
      const h = activeHours[hi]!;
      const effEnd = Math.max(h.startMs, Math.min(h.endMs - 1, to, nowMs));
      const win = Math.max(0, effEnd - h.startMs);
      const off = Math.floor(hash(daySeed * 53 + i * 19 + j * 7) * win * 0.55);
      const len = Math.floor((0.12 + hash(daySeed * 59 + i * 23 + j * 13) * 0.35) * win);
      const startMs = h.startMs + off;
      const endMs = Math.min(effEnd, startMs + len);
      spans.push({ startMs, endMs });
    }
    spans.sort((a, b) => a.startMs - b.startMs);
    return { sessionId: s.id, title: s.title, cost, requests, spans };
  });

  return sessions.sort((a, b) => b.cost - a.cost || b.requests - a.requests);
}

/**
 * Generates mock `Stats` from one unified per-hour deterministic seed for every range:
 * - Each hour bucket is included in `daily` / `overview` iff `from <= startMs <= to` and `startMs <= nowMs`, so the
 *   same full calendar day produces identical totals across 7d, 30d, 90d, and 全部;
 * - When `fromMs <= toMs` and `toMs - fromMs <= 8 days`, also emits `hourly` from those exact same hour buckets so
 *   `sum(hourly.requests) === sum(daily.requests) === overview.requests` and cost conserves;
 * - When `toMs - fromMs <= 26 hours`, also emits `hourlyModels` (conserving with `hourly`) and `sessions`.
 */
export function buildMockStats(req: StatsRequest): Stats {
  const nowMs = Date.now();
  const from = req.timeRange?.fromMs ?? null;
  const to = req.timeRange?.toMs ?? null;
  const wantHourly = from != null && to != null && from <= to && to - from <= 8 * DAY;
  const wantDayDetail = from != null && to != null && from <= to && to - from <= DAY_DETAIL_MAX_MS;

  const dailyMap = new Map<string, DailyModelTokens>();
  const addDaily = (r: DailyModelTokens) => {
    const key = `${r.day}\0${r.model}`;
    const prev = dailyMap.get(key);
    if (prev) {
      prev.input += r.input;
      prev.output += r.output;
      prev.cacheRead += r.cacheRead;
      prev.cacheCreation += r.cacheCreation;
      prev.cost += r.cost;
      prev.requests += r.requests;
    } else {
      dailyMap.set(key, { ...r });
    }
  };

  const rangeFrom = from ?? new Date(nowMs - ALL_HISTORY_DAYS * DAY).setHours(0, 0, 0, 0);
  const rangeTo = to ?? nowMs;
  const cur = new Date(rangeFrom);
  cur.setMinutes(0, 0, 0);
  const last = new Date(wantHourly ? rangeTo : Math.min(rangeTo, nowMs));
  last.setMinutes(0, 0, 0);
  const lastMs = last.getTime();

  const hourly: HourUsage[] = [];
  const hourlyModels: HourModelUsage[] = [];
  while (cur.getTime() <= lastMs) {
    const startMs = cur.getTime();
    cur.setHours(cur.getHours() + 1);
    const endMs = cur.getTime();
    if (endMs <= startMs) cur.setTime(startMs + HOUR);
    const inBounds = (from == null || startMs >= from) && (to == null || startMs <= to);
    const rows = inBounds ? hourBreakdown(startMs, nowMs) : [];
    let reqs = 0;
    let cost = 0;
    for (const r of rows) {
      addDaily(r);
      reqs += r.requests;
      cost += r.cost;
      if (wantDayDetail) {
        hourlyModels.push({
          startMs,
          model: r.model,
          input: r.input,
          output: r.output,
          cacheRead: r.cacheRead,
          cacheCreation: r.cacheCreation,
          cost: r.cost,
          requests: r.requests,
        });
      }
    }
    if (wantHourly) hourly.push({ startMs, endMs: cur.getTime(), requests: reqs, cost });
  }

  const daily = [...dailyMap.values()].sort((a, b) => a.day.localeCompare(b.day) || a.model.localeCompare(b.model));
  const overview = daily.reduce(
    (acc, d) => ({
      outputTokens: acc.outputTokens + d.output,
      cost: acc.cost + d.cost,
      unpricedMessages: acc.unpricedMessages,
      requests: acc.requests + d.requests,
    }),
    { outputTokens: 0, cost: 0, unpricedMessages: 0, requests: 0 },
  );
  const sessions = wantDayDetail ? buildMockDaySessions(hourly, overview.cost, overview.requests, from, to, nowMs) : [];
  return { overview, daily, hourly, hourlyModels, sessions };
}

/** Completion while driving (`listSlashCommands`): a few built-in commands, a project command, a plugin skill. */
export const slashCommands: SlashCommand[] = [
  { name: "add-dir", description: "添加工作目录", source: "builtin", interactive: false },
  { name: "clear", description: "清空对话，开始新的 Session", source: "builtin", interactive: false },
  { name: "compact", description: "压缩对话上下文（可附压缩说明）", source: "builtin", interactive: false },
  { name: "cost", description: "查看用量与花费", source: "builtin", interactive: true },
  { name: "model", description: "切换模型（带模型名时直接切换）", source: "builtin", interactive: true },
  { name: "rename", description: "重命名 Session", source: "builtin", interactive: false },
  { name: "release-notes", description: "查看更新说明", source: "builtin", interactive: false },
  { name: "release", description: "按仓库的发布规则发版", source: "project", interactive: false },
  { name: "kit:review", description: "审查改动", source: "plugin", interactive: false },
  { name: "help", description: "帮助与命令列表", source: "builtin", interactive: true },
  { name: "vim", description: "切换 Vim 编辑模式", source: "builtin", interactive: false },
];

/** The paths `completePaths` knows (`/add-dir` and `@` completion), as on a disk: a directory ends in `/`. */
const mockPaths = [
  "/work/demo/apps/",
  "/work/demo/apps/kestrel/",
  "/work/demo/apps/kestrel/main.go",
  "/work/demo/clusters/",
  "/work/demo/.github/",
  "/work/demo/README.md",
  "/work/demo/Cargo.toml",
  "/work/infra/",
  "/work/infra-modules/",
  "/work/notes/",
  "/Users/dev/Downloads/",
  // Where 「新建 Session」 starts (`new-session.jsonl`).
  "/tmp/mock-new/src/",
];

/** `completePaths` over `mockPaths`, as the Engine does it (relative to `cwd`, `..` up, hidden ones after a `.`). */
export function completeMockPaths(cwd: string | null, partial: string, files: boolean): string[] {
  if (partial === "~") return ["~/"];
  const cut = partial.lastIndexOf("/") + 1;
  const [head, prefix] = [partial.slice(0, cut), partial.slice(cut)];
  const from = head.startsWith("/")
    ? head
    : head.startsWith("~/")
      ? `/Users/dev/${head.slice(2)}`
      : cwd && `${cwd}/${head}`;
  if (!from) return [];
  const parts: string[] = [];
  for (const p of from.split("/")) {
    if (p === "..") parts.pop();
    else if (p && p !== ".") parts.push(p);
  }
  const base = `/${parts.join("/")}`.replace(/\/$/, "") + "/";
  const names = mockPaths
    .filter((d) => d.startsWith(base) && d.length > base.length && !d.slice(base.length, -1).includes("/"))
    .map((d) => d.slice(base.length))
    .filter((n) => files || n.endsWith("/"))
    .filter((n) => (prefix.startsWith(".") || !n.startsWith(".")) && n.toLowerCase().startsWith(prefix.toLowerCase()))
    // By name, as the Engine sorts (without the directory's `/`).
    .sort((a, b) => a.replace(/\/$/, "").toLowerCase().localeCompare(b.replace(/\/$/, "").toLowerCase()));
  if (prefix === "." || prefix === "..") names.unshift("../");
  return names.map((n) => `${head}${n}`);
}
