// Wire-replay ccrpc for the mock API (`pnpm dev:mock`, E2E): each `src/mocks/data/drive/*.jsonl` is a
// recorded `wire.jsonl` (`{t, dir: "send" | "recv", msg}`, plus `{dir: "exit", code, stderr}` and an
// optional first `{dir: "meta", ls: {...}}` that overrides the `ccrpc ls` entry) and shows up as one
// `ls` entry. A meta `session_id` (and `holder_id`) is also written into the recorded `ready`, so a
// recording can stand for a Session of CoDeck's mock data and keeps its ids when re-recorded. Connecting replays what ccrpc printed up to the first
// recorded command; each command sent that matches the next recorded one (by type) replays what
// followed it, with the response's id rewritten. Queries the recording does not have (get_state,
// get_tasks, cancel_follow_up) are answered from what was replayed. `new-session` is not listed:
// `driveNew` opens it.

import type { InMsg, LsEntry } from "@/features/drive/protocol";
import type { Api, Unlisten } from "./api";

interface WireLine {
  t?: number;
  dir: "send" | "recv" | "exit" | "meta";
  msg?: Record<string, unknown>;
  code?: number | null;
  stderr?: string;
  ls?: Partial<LsEntry>;
  /** On the meta line: `driveConnect` returns this long after the replay starts (events before the id). */
  connect_delay_ms?: number;
}

interface Fixture {
  name: string;
  lines: WireLine[];
  ls: LsEntry;
  connectDelayMs: number;
}

const raw = import.meta.glob("/src/mocks/data/drive/*.jsonl", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

function parse(name: string, text: string): Fixture {
  const lines = text
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l) as WireLine);
  const metaLine = lines.find((l) => l.dir === "meta");
  const meta = metaLine?.ls ?? {};
  const readyLine = lines.find((l) => l.dir === "recv" && l.msg?.type === "ready");
  // A meta `session_id` / `holder_id` stands in for the recorded one, so re-recording keeps the ids stable.
  if (readyLine?.msg && meta.session_id) readyLine.msg = { ...readyLine.msg, session_id: meta.session_id };
  if (readyLine?.msg && meta.holder_id) readyLine.msg = { ...readyLine.msg, holder_id: meta.holder_id };
  const ready = readyLine?.msg ?? {};
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
  return { name, lines: lines.filter((l) => l.dir !== "meta"), ls, connectDelayMs: metaLine?.connect_delay_ms ?? 0 };
}

const fixtures: Fixture[] = Object.entries(raw)
  .map(([path, text]) => parse(path.replace(/^.*\//, "").replace(/\.jsonl$/, ""), text))
  .sort((a, b) => a.name.localeCompare(b.name));

interface Conn {
  fx: Fixture;
  cursor: number;
  state: unknown;
  followUps: string[];
  closed: boolean;
}

type LineCb = (conn: string, msg: InMsg) => void;
type ExitCb = (conn: string, code: number | null, stderr: string) => void;
const lineCbs = new Set<LineCb>();
const exitCbs = new Set<ExitCb>();
const conns = new Map<string, Conn>();
let seq = 0;

/** What the E2E tests look at. */
export interface DriveProbe {
  sent: { conn: string; msg: Record<string, unknown> }[];
  /** `driveList` calls (each is a `ccrpc ls` process in the app). */
  lists: number;
  guiLog: string[];
  badge: number;
  images: number;
}
const probe: DriveProbe = { sent: [], lists: 0, guiLog: [], badge: 0, images: 0 };

declare global {
  interface Window {
    /** E2E spy on the mock ccrpc. */
    __cvDrive?: DriveProbe;
  }
}

const later = (f: () => void) => setTimeout(f, 0);

function emit(id: string, msg: InMsg) {
  const c = conns.get(id);
  if (!c || c.closed) return;
  if (msg.type === "state") c.state = msg;
  // ccrpc sends one queued follow-up per idle turn.
  if (msg.type === "turn_end") c.followUps.shift();
  for (const cb of lineCbs) cb(id, msg);
}

/** Replays from the cursor up to the next recorded command (or the end). */
function replay(id: string, rewrite?: { from: unknown; to: unknown }) {
  const c = conns.get(id);
  if (!c) return;
  const out: WireLine[] = [];
  while (c.cursor < c.fx.lines.length && c.fx.lines[c.cursor]!.dir !== "send") out.push(c.fx.lines[c.cursor++]!);
  let i = 0;
  const step = () => {
    if (i >= out.length) return;
    const l = out[i++]!;
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
    case "get_tasks":
      return respond(id, msg, { tasks: [] });
    // Typed into the TUI; what it does shows on the screen, not in a reply.
    case "slash":
      return c.state && (c.state as { state?: string }).state !== "idle" ? respond(id, msg, null, "not_idle") : respond(id, msg, {});
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
  conns.set(id, { fx, cursor: 0, state: null, followUps: [], closed: false });
  replay(id);
  return id;
}

const subscribe = <T>(set: Set<T>, cb: T): Promise<Unlisten> => {
  set.add(cb);
  return Promise.resolve(() => void set.delete(cb));
};

type DriveApi = Pick<
  Api,
  | "driveInfo"
  | "driveList"
  | "driveConnect"
  | "driveNew"
  | "driveSend"
  | "driveDisconnect"
  | "driveGuiLog"
  | "driveLogDir"
  | "driveSaveImage"
  | "pickDir"
  | "setBadge"
  | "onDriveLine"
  | "onDriveExit"
>;

export function createMockDrive(): DriveApi {
  window.__cvDrive = probe;
  return {
    driveInfo: async () => ({
      ccrpcPath: "/mock/bin/ccrpc",
      doctor: [
        { level: "ok", text: "ok\ttoh\t/mock/bin/toh" },
        { level: "fail", text: "fail\tplugin\tccrpc 插件未启用" },
      ],
      analyzer: true,
      logRoot: "/mock/Logs/dev.joy.codeck/drive",
    }),
    driveList: async () => (probe.lists++, fixtures.filter((f) => f.name !== "new-session").map((f) => ({ ...f.ls }))),
    driveConnect: async (holderId) => {
      const fx = fixtures.find((f) => f.ls.holder_id === holderId);
      if (!fx) throw new Error(`没有 ${holderId} 的录制`);
      const id = open(fx);
      // Tauri does not promise the command's answer before the connection's first events.
      if (fx.connectDelayMs) await new Promise((r) => setTimeout(r, fx.connectDelayMs));
      return id;
    },
    driveNew: async () => {
      const fx = fixtures.find((f) => f.name === "new-session");
      if (!fx) throw new Error("没有 new-session 的录制");
      return open(fx);
    },
    driveSend: async (id, m) => {
      const msg = m as Record<string, unknown>;
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
    driveDisconnect: async (id) => {
      const c = conns.get(id);
      // Like the backend: a connection whose ccrpc has exited is unknown.
      if (!c || c.closed) throw new Error("ccrpc 已退出");
      c.closed = true;
      later(() => exitCbs.forEach((cb) => cb(id, 0, "")));
    },
    driveGuiLog: async (_conn, text) => {
      probe.guiLog.push(text);
    },
    driveLogDir: async (id) => (conns.get(id)?.closed ? null : `/mock/Logs/dev.joy.codeck/drive/${id}`),
    driveSaveImage: async (_b64, ext) => `/mock/Caches/dev.joy.codeck/images/${++probe.images}.${ext}`,
    pickDir: async () => "/tmp/mock-new",
    setBadge: async (n) => {
      probe.badge = n;
    },
    onDriveLine: (cb) => subscribe(lineCbs, cb),
    onDriveExit: (cb) => subscribe(exitCbs, cb),
  };
}
