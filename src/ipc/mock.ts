// Wire-replay backend for `vite --mode mock` and the E2E tests: each `e2e/fixtures/*.jsonl` is a
// recorded `wire.jsonl` (`{t, dir: "send" | "recv", msg}`, plus `{dir: "exit", code, stderr}` and
// an optional first `{dir: "meta", ls: {...}}`) and shows up as one Session. Connecting replays
// what ccrpc printed up to the first recorded command; each command the GUI sends that matches the
// next recorded one (by type) replays what followed it, with the response's id rewritten. Queries
// the recording does not have (get_state, get_messages, …) are answered from what was replayed.

import type { Block, InMsg, LsEntry } from "@/protocol";
import type { AppInfo, Backend } from "./api";

interface WireLine {
  t?: number;
  dir: "send" | "recv" | "exit" | "meta";
  msg?: Record<string, unknown>;
  code?: number | null;
  stderr?: string;
  ls?: Partial<LsEntry>;
}

interface Fixture {
  name: string;
  lines: WireLine[];
  ls: LsEntry;
}

const raw = import.meta.glob("/e2e/fixtures/*.jsonl", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

function parse(name: string, text: string): Fixture {
  const lines = text
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l) as WireLine);
  const ready = lines.find((l) => l.dir === "recv" && l.msg?.type === "ready")?.msg ?? {};
  const meta = lines.find((l) => l.dir === "meta")?.ls ?? {};
  const ls: LsEntry = {
    holder_id: String(ready.holder_id ?? name),
    session_id: (ready.session_id as string | null) ?? null,
    status: "idle",
    cwd: (ready.cwd as string | null) ?? `/mock/${name}`,
    plugin: true,
    claude_pid: null,
    version: (ready.claude_version as string | null) ?? null,
    title: name,
    ...meta,
  };
  return { name, lines: lines.filter((l) => l.dir !== "meta"), ls };
}

const fixtures: Fixture[] = Object.entries(raw)
  .map(([path, text]) => parse(path.replace(/^.*\//, "").replace(/\.jsonl$/, ""), text))
  .sort((a, b) => a.name.localeCompare(b.name));

interface Conn {
  fx: Fixture;
  cursor: number;
  state: unknown;
  blocks: Block[];
  followUps: string[];
  closed: boolean;
}

type LineCb = (conn: string, msg: InMsg) => void;
type ExitCb = (conn: string, code: number | null, stderr: string) => void;
const lineCbs = new Set<LineCb>();
const exitCbs = new Set<ExitCb>();
const conns = new Map<string, Conn>();
let seq = 0;

/** What the tests look at. */
export interface MockProbe {
  sent: { conn: string; msg: Record<string, unknown> }[];
  guiLog: string[];
  badge: number;
  images: number;
}
const probe: MockProbe = { sent: [], guiLog: [], badge: 0, images: 0 };
if (typeof window !== "undefined") (window as unknown as { __mock: MockProbe }).__mock = probe;

const later = (f: () => void) => setTimeout(f, 0);

function emit(id: string, msg: InMsg) {
  const c = conns.get(id);
  if (!c || c.closed) return;
  if (msg.type === "state") c.state = msg;
  if (msg.type === "block" && !(msg as Block).agent_id) c.blocks.push(msg as Block);
  if (msg.type === "turn_end") c.followUps = [];
  for (const cb of lineCbs) cb(id, msg);
}

/** Replays from the cursor up to the next recorded command (or the end). */
function replay(id: string, rewrite?: { from: unknown; to: unknown }) {
  const c = conns.get(id);
  if (!c) return;
  const out: WireLine[] = [];
  while (c.cursor < c.fx.lines.length && c.fx.lines[c.cursor].dir !== "send") out.push(c.fx.lines[c.cursor++]);
  let i = 0;
  const step = () => {
    if (i >= out.length) return;
    const l = out[i++];
    if (l.dir === "exit") {
      c.closed = true;
      for (const cb of exitCbs) cb(id, l.code ?? null, l.stderr ?? "");
      return;
    }
    let msg = l.msg as unknown as InMsg;
    if (rewrite && msg.type === "response" && msg.id === rewrite.from) msg = { ...msg, id: rewrite.to as string };
    emit(id, msg);
    later(step);
  };
  later(step);
}

function respond(id: string, msg: Record<string, unknown>, data: Record<string, unknown> | null, error?: string) {
  const r: InMsg = {
    type: "response",
    id: msg.id as string,
    command: String(msg.type),
    success: error === undefined,
    ...(error === undefined ? { data: data ?? {} } : { error: { code: "mock", message: error } }),
  };
  later(() => emit(id, r));
}

function synth(id: string, c: Conn, msg: Record<string, unknown>) {
  switch (msg.type) {
    case "get_state": {
      const s = (c.state ?? { state: "idle" }) as Record<string, unknown>;
      return respond(id, msg, { ...s, follow_ups: c.followUps, follow_up_queue: c.followUps.length });
    }
    case "get_messages":
      return respond(id, msg, {
        blocks: msg.before != null ? [] : c.blocks.slice(-Number(msg.limit ?? 1e9)),
        cursor: c.blocks.length,
        first: msg.before != null || c.blocks.length === 0 ? null : 0,
        more: false,
        ...(msg.subagents ? { subagent_blocks: [] } : {}),
      });
    case "get_tasks":
      return respond(id, msg, { tasks: [] });
    case "get_screen":
      return respond(id, msg, { lines: [], cursor: [0, 0] });
    case "get_tool_result": {
      const r = c.blocks.find((b) => b.kind === "tool_result" && b.tool_use_id === msg.tool_use_id);
      return r
        ? respond(id, msg, { content: (r as { content: string }).content, persisted: null })
        : respond(id, msg, null, "录制里没有这个工具结果");
    }
    case "cancel_follow_up": {
      const i = msg.index as number | undefined;
      const removed = i == null ? c.followUps.length : 1;
      c.followUps = i == null ? [] : c.followUps.filter((_, j) => j !== i);
      return respond(id, msg, { removed, follow_ups: c.followUps });
    }
    default:
      return respond(id, msg, null, `录制里没有 ${String(msg.type)}`);
  }
}

function open(fx: Fixture): string {
  const id = `mock${++seq}`;
  conns.set(id, { fx, cursor: 0, state: null, blocks: [], followUps: [], closed: false });
  replay(id);
  return id;
}

export const mockBackend: Backend = {
  appInfo: async (): Promise<AppInfo> => ({
    ccrpcPath: "/mock/ccrpc",
    doctor: [{ level: "ok", text: "ok\tmock" }],
    analyzer: true,
    logRoot: "/mock/logs",
  }),
  listSessions: async () => fixtures.filter((f) => f.name !== "new-session").map((f) => f.ls),
  connect: async (holderId) => {
    const fx = fixtures.find((f) => f.ls.holder_id === holderId);
    if (!fx) throw new Error(`没有 ${holderId} 的录制`);
    return open(fx);
  },
  newSession: async () => {
    const fx = fixtures.find((f) => f.name === "new-session");
    if (!fx) throw new Error("没有 new-session 的录制");
    return open(fx);
  },
  send: async (id, msg) => {
    probe.sent.push({ conn: id, msg });
    const c = conns.get(id);
    if (!c || c.closed) throw new Error("ccrpc 已退出");
    if (msg.type === "follow_up") c.followUps.push(String(msg.text));
    const next = c.fx.lines[c.cursor];
    if (next?.dir === "send" && next.msg?.type === msg.type) {
      c.cursor++;
      replay(id, { from: next.msg?.id, to: msg.id });
      return;
    }
    synth(id, c, msg);
  },
  disconnect: async (id) => {
    const c = conns.get(id);
    if (c) c.closed = true;
  },
  guiLog: async (_conn, text) => {
    probe.guiLog.push(text);
  },
  saveImage: async (_b64, ext) => `/tmp/cc-gui-mock-${++probe.images}.${ext}`,
  pickDir: async () => "/tmp/mock-new",
  setBadge: async (n) => {
    probe.badge = n;
  },
  copyText: async () => {},
  onLine: (cb) => {
    lineCbs.add(cb);
    return () => lineCbs.delete(cb);
  },
  onExit: (cb) => {
    exitCbs.add(cb);
    return () => exitCbs.delete(cb);
  },
};
