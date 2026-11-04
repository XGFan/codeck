import type {
  Activity,
  ActivityBucket,
  AppError,
  AppInfo,
  BranchChoice,
  FindMatch,
  FindRequest,
  FindResult,
  IndexStatus,
  JumpRequest,
  JumpTarget,
  LiveChanged,
  LiveState,
  Node,
  ProjectSummary,
  SearchGroup,
  SearchHit,
  SearchRequest,
  SearchResponse,
  SearchRole,
  SessionDetail,
  SessionQuery,
  SessionsChanged,
  SessionSummary,
  SnippetPart,
  Stats,
  StatsRequest,
  ToolCall,
  ToolDetail,
  Transcript,
  TranscriptRequest,
  TranscriptScope,
} from "./bindings";
import type { Api, Unlisten } from "./api";
import { createMockCcrpc } from "./mock-ccrpc";
import { DAY, NOW, sumTok, tok, TINY_PNG_BASE64 } from "@/mocks/data/builders";
import { appInfo, diagnostics, indexStatus as indexStatusSeed, slashCommands, stats as statsSeed } from "@/mocks/data/misc";
import { parseQuery } from "@/features/search/match";
import { projectBase } from "@/mocks/data/projects";
import {
  PERSISTED_FULL,
  RICH_ID,
  richAgentNodes,
  richAlt,
  richAliases,
  richBranch,
  richSubagents,
  richWorkflows,
} from "@/mocks/data/rich";
import { richModelCosts, sessionData } from "@/mocks/data/sessions";

const LATENCY_MS = 30;

type EventName = "sessionsChanged" | "liveChanged" | "indexStatus" | "pricesChanged";

declare global {
  interface Window {
    /** E2E hooks, installed only by the mock API. */
    /** E2E spy: clipboard / Finder calls and search queries (args: [query]) made through the mock API. */
    __cvCalls?: Array<{ method: "copyText" | "revealSessionFile" | "search" | "findInSession"; args: unknown[] }>;
    __cvMock?: {
      emit(eventName: string, payload: unknown): void;
      setLive(sessionId: string, state: LiveState | null): void;
      appendNode(sessionId: string, node: Node): void;
    };
  }
}

const appError = (code: AppError["code"], message: string): AppError => ({ code, message });

/** Local calendar day `YYYY-MM-DD`. */
function localDay(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

// ---- search documents -------------------------------------------------------------------------

interface Doc {
  sessionId: string;
  nodeId: string;
  agentId: string | null;
  toolUseId: string | null;
  role: SearchRole;
  timestampMs: number | null;
  onMainLine: boolean;
  text: string;
  hidden: boolean;
}

/** AskUserQuestion answers and notes are searchable as the user's words. */
interface AskExtra {
  questions?: unknown;
  answers?: Record<string, string>;
  annotations?: Record<string, { notes?: string }>;
}

function docsOf(sessionId: string, nodes: Node[], agentId: string | null, onMainLine: boolean): Doc[] {
  const out: Doc[] = [];
  const push = (n: Node, role: SearchRole, text: string, toolUseId: string | null = null) =>
    out.push({ sessionId, nodeId: n.id, agentId, toolUseId, role, timestampMs: n.timestampMs, onMainLine, text, hidden: n.hidden });
  for (const n of nodes) {
    const b = n.body;
    if (b.kind === "userPrompt") push(n, "user", b.text);
    else if (b.kind === "assistant") {
      for (const blk of b.blocks) {
        if (blk.kind === "text" || blk.kind === "thinking") push(n, "assistant", blk.text);
        else if (blk.kind === "toolCall") {
          push(n, "toolInput", blk.inputJson, blk.toolUseId);
          const ask = blk.result?.extraJson ? (JSON.parse(blk.result.extraJson) as AskExtra) : null;
          if (ask?.questions && ask.answers) {
            const notes = Object.values(ask.annotations ?? {}).map((a) => a.notes ?? "");
            push(n, "user", [...Object.values(ask.answers), ...notes].filter(Boolean).join("\n"), blk.toolUseId);
          }
        }
      }
    }
  }
  return out;
}

// ---- mutable state ------------------------------------------------------------------------------

const appended = new Map<string, Node[]>();
const revisions = new Map<string, number>();
const liveOverrides = new Map<string, LiveState | null>();
let currentIndexStatus: IndexStatus = { ...indexStatusSeed };

const listeners: { [K in EventName]: Set<(p: never) => void> } = {
  sessionsChanged: new Set(),
  liveChanged: new Set(),
  indexStatus: new Set(),
  pricesChanged: new Set(),
};

function dispatch(name: EventName, payload: unknown) {
  for (const cb of listeners[name]) (cb as (p: unknown) => void)(payload);
}

function normalizeEventName(name: string): EventName {
  const n = name.replace(/-event$/, "").replace(/-(\w)/g, (_, c: string) => c.toUpperCase());
  if (n === "sessionsChanged" || n === "liveChanged" || n === "indexStatus") return n;
  throw new Error(`unknown mock event: ${name}`);
}

const findData = (sessionId: string) => {
  const d = sessionData.find((s) => s.summary.id === sessionId);
  if (!d) throw appError("notFound", "会话不存在");
  return d;
};

const mainNodes = (sessionId: string): Node[] => [...findData(sessionId).nodes, ...(appended.get(sessionId) ?? [])];

function summaryOf(sessionId: string): SessionSummary {
  const d = findData(sessionId);
  const extra = appended.get(sessionId)?.length ?? 0;
  const live = liveOverrides.has(sessionId) ? (liveOverrides.get(sessionId) ?? null) : d.summary.live;
  return {
    ...d.summary,
    messageCount: d.summary.messageCount + extra,
    lastActiveMs: extra ? NOW : d.summary.lastActiveMs,
    live,
  };
}

function allDocs(): Doc[] {
  return sessionData.flatMap((d) => {
    const id = d.summary.id;
    const docs = docsOf(id, mainNodes(id), null, true);
    if (id === RICH_ID) {
      docs.push(...docsOf(id, richAlt, null, false));
      for (const [agentId, nodes] of Object.entries(richAgentNodes)) docs.push(...docsOf(id, nodes, agentId, true));
    }
    return docs;
  });
}

// ---- transcript ---------------------------------------------------------------------------------

function selectPath(sessionId: string, choices: BranchChoice[]): { nodes: Node[]; selectedHeadId: string | null } {
  const nodes = mainNodes(sessionId);
  if (sessionId !== RICH_ID) return { nodes, selectedHeadId: null };
  const idx = nodes.findIndex((n) => n.id === richBranch.mainHeadId);
  const pickedAlt = choices.some((c) => c.anchorKey === richBranch.anchorKey && c.headId === richBranch.altHeadId);
  if (pickedAlt) return { nodes: [...nodes.slice(0, idx), ...richAlt], selectedHeadId: richBranch.altHeadId };
  return { nodes, selectedHeadId: richBranch.mainHeadId };
}

function agentChain(agentId: string): string[] {
  const chain: string[] = [];
  let cur = richSubagents.find((a) => a.agentId === agentId);
  while (cur) {
    chain.unshift(cur.agentId);
    const parent: string | null = cur.parentAgentId;
    cur = parent ? richSubagents.find((a) => a.agentId === parent) : undefined;
  }
  return chain;
}

function buildTranscript(req: { sessionId: string; scope: TranscriptScope; branchChoices: BranchChoice[]; includeHidden: boolean }): Transcript {
  const d = findData(req.sessionId);
  const revision = (revisions.get(req.sessionId) ?? 0) + 1;
  const isRich = req.sessionId === RICH_ID;
  let nodes: Node[];
  let branchPoints: Transcript["branchPoints"] = [];
  if (req.scope.kind === "subagent") {
    const agentNodes = isRich ? richAgentNodes[req.scope.agentId] : undefined;
    if (!agentNodes) throw appError("notFound", "子代理不存在");
    nodes = agentNodes;
  } else {
    const sel = selectPath(req.sessionId, req.branchChoices);
    nodes = sel.nodes;
    if (isRich && sel.selectedHeadId) {
      branchPoints = [{ anchorKey: richBranch.anchorKey, selectedHeadId: sel.selectedHeadId, options: richBranch.options }];
    }
  }
  const hiddenCount = nodes.filter((n) => n.hidden).length;
  const visible = req.includeHidden ? nodes : nodes.filter((n) => !n.hidden);
  const inherited =
    req.scope.kind === "main" && d.forkOf
      ? {
          originSessionId: d.forkOf.id,
          originTitle: findData(d.forkOf.id).summary.title,
          lastInheritedId: d.forkOf.lastInheritedId,
          count: d.forkOf.count,
        }
      : null;
  return {
    sessionId: req.sessionId,
    scope: req.scope,
    revision,
    nodes: visible,
    branchPoints,
    hiddenCount,
    inherited,
    subagents: isRich ? richSubagents : [],
    workflows: isRich ? richWorkflows : [],
    orphanSubagentIds: isRich ? ["ag-orphan"] : [],
  };
}

function toolCalls(sessionId: string): Map<string, ToolCall> {
  const m = new Map<string, ToolCall>();
  const scan = (nodes: Node[]) => {
    for (const n of nodes)
      if (n.body.kind === "assistant") for (const b of n.body.blocks) if (b.kind === "toolCall") m.set(b.toolUseId, b);
  };
  scan(mainNodes(sessionId));
  if (sessionId === RICH_ID) {
    scan(richAlt);
    Object.values(richAgentNodes).forEach(scan);
  }
  return m;
}

// ---- search -------------------------------------------------------------------------------------

function snippet(text: string, terms: string[]): SnippetPart[] {
  const lower = text.toLowerCase();
  const first = Math.min(...terms.map((t) => lower.indexOf(t)).filter((i) => i >= 0));
  const from = Math.max(0, first - 24);
  const slice = text.slice(from, from + 120);
  const prefix = from > 0 ? "…" : "";
  const re = new RegExp(`(${terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "gi");
  const parts: SnippetPart[] = [];
  slice.split(re).forEach((s, i) => {
    if (s) parts.push({ text: i === 0 ? prefix + s : s, hit: i % 2 === 1 });
  });
  return parts;
}

function searchGroups(req: SearchRequest): SearchGroup[] {
  const { include, exclude } = parseQuery(req.query);
  if (include.length === 0) return [];
  const bySession = new Map<string, SearchHit[]>();
  for (const doc of allDocs()) {
    if (doc.hidden || !DEFAULT_ROLES.includes(doc.role)) continue;
    const s = summaryOf(doc.sessionId);
    if (req.projectId && !relatedProjects(s).includes(req.projectId)) continue;
    const lower = doc.text.toLowerCase();
    if (!include.every((t) => lower.includes(t)) || exclude.some((t) => lower.includes(t))) continue;
    const hits = bySession.get(doc.sessionId) ?? [];
    hits.push({
      nodeId: doc.nodeId,
      agentId: doc.agentId,
      agentType: doc.agentId ? (richSubagents.find((a) => a.agentId === doc.agentId)?.agentType ?? null) : null,
      toolUseId: doc.toolUseId,
      role: doc.role,
      timestampMs: doc.timestampMs,
      onMainLine: doc.onMainLine,
      snippet: snippet(doc.text, include),
    });
    bySession.set(doc.sessionId, hits);
  }
  return [...bySession.entries()]
    .map(([id, hits]) => ({ session: summaryOf(id), hitCount: hits.length, hits: hits.slice(0, req.hitsPerSession || 5) }))
    .sort((a, b) => b.session.lastActiveMs - a.session.lastActiveMs)
    .slice(0, req.maxSessions || 50);
}

/** Searched roles: tool output is not indexed (ADR-0004). */
const DEFAULT_ROLES: SearchRole[] = ["user", "assistant", "toolInput"];

// ---- misc ---------------------------------------------------------------------------------------

const countOf = (hay: string, needle: string) => {
  if (!needle) return 0;
  const h = hay.toLowerCase();
  const n = needle.toLowerCase();
  let c = 0;
  for (let i = h.indexOf(n); i >= 0; i = h.indexOf(n, i + n.length)) c++;
  return c;
};

function findIn(req: FindRequest): FindResult {
  const tr = buildTranscript(req);
  const matches: FindMatch[] = [];
  const add = (nodeId: string, location: FindMatch["location"], hay: string) => {
    const count = countOf(hay, req.query);
    if (count) matches.push({ nodeId, location, count });
  };
  for (const n of tr.nodes) {
    const b = n.body;
    if (b.kind === "userPrompt" || b.kind === "compactSummary" || b.kind === "system" || b.kind === "attachment") add(n.id, { kind: "text" }, b.text);
    else if (b.kind === "assistant")
      for (const blk of b.blocks) {
        if (blk.kind === "text") add(n.id, { kind: "text" }, blk.text);
        else if (blk.kind === "thinking") add(n.id, { kind: "thinking" }, blk.text);
        else if (blk.kind === "toolCall") {
          add(n.id, { kind: "toolInput", toolUseId: blk.toolUseId }, blk.inputJson);
          if (blk.result) add(n.id, { kind: "toolOutput", toolUseId: blk.toolUseId }, blk.result.text);
        }
      }
  }
  return { matches, total: matches.reduce((s, m) => s + m.count, 0) };
}

function resolveJump(req: JumpRequest): JumpTarget {
  const alias = richAliases[req.nodeId];
  const nodeId = alias?.nodeId ?? req.nodeId;
  const toolUseId = req.toolUseId ?? alias?.toolUseId ?? null;
  const doc = allDocs().find((d) => d.sessionId === req.sessionId && d.nodeId === nodeId && (req.agentId ? d.agentId === req.agentId : d.agentId === null));
  if (!doc) throw appError("notFound", "找不到目标消息");
  const agentPath = doc.agentId ? agentChain(doc.agentId) : [];
  const abandoned = !doc.onMainLine;
  return {
    sessionId: req.sessionId,
    scope: doc.agentId ? { kind: "subagent", agentId: doc.agentId } : { kind: "main" },
    branchChoices: abandoned ? [{ anchorKey: richBranch.anchorKey, headId: richBranch.altHeadId }] : [],
    nodeId,
    toolUseId,
    agentPath,
    inSubagent: agentPath.length > 0,
    inAbandonedBranch: abandoned,
    needsHidden: doc.hidden,
  };
}

/** The Project a session started in and every Project it edited. */
function relatedProjects(s: SessionSummary): string[] {
  return [s.projectId, ...s.editedProjectIds];
}

function listSessions(q: SessionQuery): SessionSummary[] {
  let rows = sessionData.map((d) => ({ d, s: summaryOf(d.summary.id) }));
  if (q.projectIds.length) rows = rows.filter(({ s }) => relatedProjects(s).some((p) => q.projectIds.includes(p)));
  if (q.timeRange) {
    const { fromMs, toMs } = q.timeRange;
    rows = rows.filter(({ s }) => (fromMs == null || s.lastActiveMs >= fromMs) && (toMs == null || s.lastActiveMs <= toMs));
  }
  const drill = q.drill;
  if (drill) {
    rows = rows.filter(({ s }) => {
      switch (drill.kind) {
        case "day":
          return localDay(s.lastActiveMs) === drill.day;
        case "model":
          return s.primaryModel === drill.model;
        case "span":
          return s.lastActiveMs >= drill.fromMs && s.createdMs < drill.toMs;
      }
    });
  }
  const key = (s: SessionSummary) =>
    q.sort === "lastActive"
      ? s.lastActiveMs
      : q.sort === "created"
        ? s.createdMs
        : q.sort === "messages"
          ? s.messageCount
          : q.sort === "cost"
            ? s.cost
            : s.tokens.output;
  rows.sort((a, b) => (q.descending ? key(b.s) - key(a.s) : key(a.s) - key(b.s)));
  return rows.map(({ s }) => s);
}

function projects(): ProjectSummary[] {
  return projectBase.map((p) => {
    const ss = sessionData.map((d) => summaryOf(d.summary.id)).filter((s) => relatedProjects(s).includes(p.id));
    return {
      ...p,
      sessionCount: ss.length,
      liveCount: ss.filter((s) => s.live).length,
      lastActiveMs: Math.max(0, ...ss.map((s) => s.lastActiveMs)),
    };
  });
}

function sessionDetail(sessionId: string): SessionDetail {
  const s = summaryOf(sessionId);
  const p = projectBase.find((x) => x.id === s.projectId)!;
  const isRich = sessionId === RICH_ID;
  const sub = isRich ? sumTok(...richSubagents.map((a) => a.tokens)) : tok(0, 0);
  return {
    summary: s,
    cwd: p.path,
    cwdMissing: p.missing,
    projectPath: p.path,
    projectMissing: p.missing,
    durationMs: s.lastActiveMs - s.createdMs,
    models: [...new Set([s.primaryModel, ...(isRich ? richSubagents.map((a) => a.model) : [])].filter((m): m is string => !!m))],
    tokensMain: { input: s.tokens.input - sub.input, output: s.tokens.output - sub.output, cacheRead: s.tokens.cacheRead - sub.cacheRead, cacheCreation: s.tokens.cacheCreation - sub.cacheCreation },
    tokensSubagents: sub,
    modelCosts: isRich
      ? richModelCosts()
      : s.primaryModel
        ? [{ model: s.primaryModel, cost: s.cost, unpricedMessages: s.unpricedMessages }]
        : s.unpricedMessages
          ? [{ model: "unknown", cost: 0, unpricedMessages: s.unpricedMessages }]
          : [],
    versions: ["2.1.91"],
    files: [
      { path: `/Users/dev/.claude/projects/${s.projectId}/${sessionId}.jsonl`, size: 1_482_112, role: "main" },
      ...(isRich ? richSubagents.map((a) => ({ path: `/Users/dev/.claude/projects/${s.projectId}/${sessionId}/subagents/agent-${a.agentId}.jsonl`, size: 48_210, role: a.workflowRunId ? ("workflowSubagent" as const) : ("subagent" as const) })) : []),
    ],
    forks: sessionData
      .filter((x) => x.forkOf?.id === sessionId)
      .map((x) => ({ sessionId: x.summary.id, title: x.summary.title, createdMs: x.summary.createdMs })),
    failedLines: 0,
    resumeCommand: `claude --resume ${sessionId}`,
  };
}

const HOUR = DAY / 24;
const ACTIVITY_BUCKETS: [ActivityBucket, number][] = [
  ["hour1", HOUR],
  ["hour3", 3 * HOUR],
  ["hour6", 6 * HOUR],
  ["hour12", 12 * HOUR],
  ["day", DAY],
  ["week", 7 * DAY],
  ["month", 30 * DAY],
];

/** Same bucket rule as `stats::choose_bucket` (≤ 170 bars, local alignment), with made-up counts. */
function mockActivity(fromMs: number | null, toMs: number | null): Activity {
  const to = toMs ?? NOW;
  const from = fromMs ?? NOW - 400 * DAY;
  const [bucket, len] = ACTIVITY_BUCKETS.find(([, l]) => Math.floor((to - from) / l) + 2 <= 170) ?? ACTIVITY_BUCKETS[6]!;
  const t = new Date(from);
  t.setMinutes(0, 0, 0);
  if (len < DAY) t.setHours(t.getHours() - (t.getHours() % (len / HOUR)));
  else t.setHours(0);
  if (bucket === "week") t.setDate(t.getDate() - ((t.getDay() + 6) % 7));
  if (bucket === "month") t.setDate(1);
  const bars: Activity["bars"] = [];
  for (let i = 0; t.getTime() <= to; i++) {
    const start = t.getTime();
    if (bucket === "month") t.setMonth(t.getMonth() + 1);
    else if (len >= DAY) t.setDate(t.getDate() + len / DAY);
    else t.setHours(t.getHours() + len / HOUR);
    const h = new Date(start).getHours();
    const awake = len >= DAY || (h >= 9 && h <= 23) ? 1 : 0.05;
    bars.push({ startMs: start, endMs: t.getTime(), messages: Math.round(awake * (Math.abs(Math.sin(i * 12.9898)) * 40) * (len / HOUR) ** 0.5) });
  }
  return { bucket, bars };
}

function getStats(req: StatsRequest): Stats {
  const from = req.timeRange?.fromMs ?? null;
  const to = req.timeRange?.toMs ?? null;
  const inRange = (day: string) => {
    const ms = new Date(`${day}T12:00:00`).getTime();
    return (from == null || ms >= from - DAY / 2) && (to == null || ms <= to + DAY / 2);
  };
  const daily = statsSeed.daily.filter((d) => inRange(d.day));
  const ranked = sessionData
    .map((d) => summaryOf(d.summary.id))
    .filter((s) => s.messageCount > 0)
    .sort((a, b) => b.cost - a.cost)
    .slice(0, 10)
    .map((s) => ({ sessionId: s.id, title: s.title, messages: s.messageCount, outputTokens: s.tokens.output, cost: s.cost }));
  return {
    ...statsSeed,
    // Fixed totals: the seeded charts are not derived from the mock sessions.
    overview: {
      sessions: 10,
      messages: 1286,
      outputTokens: daily.reduce((s, d) => s + d.output, 0),
      activeDays: new Set(daily.map((d) => d.day)).size,
      cost: 412.37,
      unpricedMessages: 0,
    },
    daily,
    heatDaily: statsSeed.heatDaily.filter((d) => inRange(d.day)),
    sessions: ranked,
    activity: mockActivity(from, to),
  };
}

const later = <T>(fn: () => T): Promise<T> =>
  new Promise((resolve, reject) =>
    setTimeout(() => {
      try {
        resolve(structuredClone(fn()));
      } catch (e) {
        reject(e);
      }
    }, LATENCY_MS),
  );

function toolDetail(sessionId: string, toolUseId: string, part: "input" | "output"): ToolDetail {
  const call = toolCalls(sessionId).get(toolUseId);
  if (!call) throw appError("notFound", "工具调用不存在");
  if (part === "input") {
    const text = JSON.stringify(JSON.parse(call.inputJson), null, 2);
    return { text, truncated: false, totalBytes: text.length, source: "inline" };
  }
  if (call.result?.persisted) {
    return { text: PERSISTED_FULL, truncated: false, totalBytes: call.result.persisted.sizeBytes, source: "persistedFile" };
  }
  if (!call.result) return { text: "", truncated: false, totalBytes: 0, source: "missing" };
  return { text: call.result.text, truncated: false, totalBytes: call.result.totalBytes, source: "inline" };
}

export function createMockApi(): Api {
  const subscribe = <T>(name: EventName, cb: (p: T) => void): Promise<Unlisten> => {
    listeners[name].add(cb as (p: never) => void);
    return Promise.resolve(() => listeners[name].delete(cb as (p: never) => void));
  };

  const liveSnapshot = (): LiveChanged => ({
    live: sessionData.flatMap((d) => {
      const live = summaryOf(d.summary.id).live;
      return live ? [{ sessionId: d.summary.id, state: live }] : [];
    }),
  });

  const calls: NonNullable<Window["__cvCalls"]> = [];
  window.__cvCalls = calls;
  window.__cvMock = {
    emit(eventName, payload) {
      const name = normalizeEventName(eventName);
      if (name === "indexStatus") currentIndexStatus = payload as IndexStatus;
      dispatch(name, payload);
    },
    setLive(sessionId, state) {
      findData(sessionId);
      liveOverrides.set(sessionId, state);
      dispatch("liveChanged", liveSnapshot());
    },
    appendNode(sessionId, node) {
      findData(sessionId);
      appended.set(sessionId, [...(appended.get(sessionId) ?? []), node]);
      revisions.set(sessionId, (revisions.get(sessionId) ?? 0) + 1);
    },
  };

  return {
    getAppInfo: () => later((): AppInfo => appInfo),
    setDataRoot: (path) => later((): AppInfo => ({ ...appInfo, dataRoot: path ?? appInfo.dataRoot, dataRootSource: path ? "settings" : "default" })),
    getIndexStatus: () => later(() => currentIndexStatus),
    rebuildIndex: () =>
      later(() => {
        const total = indexStatusSeed.filesTotal;
        dispatch("indexStatus", { ...indexStatusSeed, phase: "scanning", filesDone: 0, textReady: false } satisfies IndexStatus);
        setTimeout(() => {
          currentIndexStatus = { ...indexStatusSeed, filesDone: total };
          dispatch("indexStatus", currentIndexStatus);
        }, 300);
        return null;
      }),
    listProjects: () => later(projects),
    listSessions: (q) => later(() => listSessions(q)),
    getSession: (id) => later(() => sessionDetail(id)),
    getTranscript: (req: TranscriptRequest) => later(() => buildTranscript(req)),
    getToolDetail: (req) => later(() => toolDetail(req.sessionId, req.toolUseId, req.part)),
    getImage: (req) => later(() => ({ mediaType: req.image.mediaType, dataBase64: TINY_PNG_BASE64 })),
    search: (req) =>
      later((): SearchResponse => {
        calls.push({ method: "search", args: [req.query, req.projectId] });
        const groups = searchGroups({ ...req, maxSessions: req.maxSessions || 50 });
        return {
          groups,
          totalSessions: groups.length,
          totalHits: groups.reduce((s, g) => s + g.hitCount, 0),
          mode: "fts",
          elapsedMs: 12,
          indexComplete: currentIndexStatus.textReady,
        };
      }),
    resolveJump: (req) => later(() => resolveJump(req)),
    findInSession: (req) =>
      later(() => {
        calls.push({ method: "findInSession", args: [req.query] });
        return findIn(req);
      }),
    revealSessionFile: (sessionId, agentId) =>
      later(() => {
        calls.push({ method: "revealSessionFile", args: [sessionId, agentId] });
        return null;
      }),
    getStats: (req) => later(() => getStats(req)),
    getDiagnostics: () => later(() => diagnostics),
    listSlashCommands: () => later(() => slashCommands),
    onIndexStatus: (cb) => subscribe("indexStatus", cb),
    onSessionsChanged: (cb) => subscribe<SessionsChanged>("sessionsChanged", cb),
    onLiveChanged: (cb) => subscribe("liveChanged", cb),
    onPricesChanged: (cb) => subscribe("pricesChanged", cb),
    copyText: async (text) => {
      calls.push({ method: "copyText", args: [text] });
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        /* clipboard unavailable in headless browsers */
      }
    },
    ...createMockCcrpc(),
  };
}
