// Wire-replay ccrpc for the mock API (`pnpm dev:mock`, E2E): each `src/mocks/data/connect/*.jsonl` is a
// recorded `wire.jsonl` (`{t, dir: "send" | "recv", msg}`, plus `{dir: "exit", code, stderr}` and an
// optional first `{dir: "meta", ls: {...}}` that overrides the `ccrpc ls` entry) and shows up as one
// `ls` entry. A meta `session_id` (and `holder_id`) is also written into the recorded `ready`, so a
// recording can stand for a Session of CoDeck's mock data and keeps its ids when re-recorded. Connecting replays what ccrpc printed up to the first
// recorded command; each command sent that matches the next recorded one (by type) replays what
// followed it, with the response's id rewritten. Queries the recording does not have (get_state,
// get_tasks) are answered from what was replayed. `new-session` and `resume*` are not listed: `ccrpcNew` opens
// the first; `ccrpcResume` opens `resume` (its `session_changed` names the Session asked for), or
// `resume-failed` for the Session `RESUME_FAILS` (claude cannot find it). `fork*` are not listed either: `ccrpcFork`
// opens `fork` (before a prompt), `fork-session` (the whole Session), `fork-warning` (restoring code: the rewind's ⚠
// confirm card first) or `fork-failed` (once a test set `forkFails`). A recording whose meta has `on_request`
// stands for a Session other tests need out of `ccrpc ls`: it is listed once a test (or the console) calls
// `__cvCcrpc.list(name)`. `__cvCcrpc.remoteConn(name)` and `remoteDisconnect(conn)` stand for an iPhone on the LAN; what
// has to be there before the app starts (`snapshot`, `lan`) comes from an init script's `window.__cvCcrpcPreset`.

import type { InMsg, LsEntry, Task } from "@/features/connect/protocol";
import { editable } from "@/features/connect/queue";
import type { Api, Unlisten } from "./api";
import type { AnalyzerInfo, AppError, LanStatus, LaunchRule } from "./bindings";

/** A rejected IPC call as the backend rejects it (`AppError`, not an `Error`). */
export const appError = (code: AppError["code"], message: string): AppError => ({ code, message });

interface WireLine {
  t?: number;
  dir: "send" | "recv" | "exit" | "meta";
  msg?: Record<string, unknown>;
  code?: number | null;
  stderr?: string;
  ls?: Partial<LsEntry>;
  /** On the meta line: `ccrpcConnect` returns this long after the replay starts (events before the id). */
  connect_delay_ms?: number;
  /** On the meta line: what `get_tasks` answers (the recording's background tasks). */
  tasks?: Task[];
  /** On the meta line: listed in `ccrpc ls` only once asked for (`__cvCcrpc.list(name)`). */
  on_request?: boolean;
}

interface Fixture {
  name: string;
  lines: WireLine[];
  ls: LsEntry;
  connectDelayMs: number;
  tasks: Task[];
  onRequest: boolean;
}

const raw = import.meta.glob("/src/mocks/data/connect/*.jsonl", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

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
    hidden: false,
    claude_pid: null,
    version: (ready.claude_version as string | null) ?? null,
    title: name,
    ...meta,
  };
  return {
    name,
    lines: lines.filter((l) => l.dir !== "meta"),
    ls,
    connectDelayMs: metaLine?.connect_delay_ms ?? 0,
    tasks: metaLine?.tasks ?? [],
    onRequest: !!metaLine?.on_request,
  };
}

const fixtures: Fixture[] = Object.entries(raw)
  .map(([path, text]) => parse(path.replace(/^.*\//, "").replace(/\.jsonl$/, ""), text))
  .sort((a, b) => a.name.localeCompare(b.name));

interface Conn {
  fx: Fixture;
  cursor: number;
  state: unknown;
  /** The last `session_info` (or `ready`): what a synthesized one changes. */
  info: Record<string, unknown>;
  closed: boolean;
  /** Tasks `stop_task` stopped on this connection: `get_tasks` reports them stopped. */
  stopped: Set<string>;
  /** The TUI's queue as the last `queue` event said: what `edit_queue` takes back. */
  queue?: string[];
  /** What follows `ready` waits this long (claude still starting). */
  startMs?: number;
  /** Started by a client on the LAN: a Connect to the same Holder gets this connection. */
  remote?: boolean;
}

/** What `/model <alias>` sets in the mock: the display name ccrpc reports at once, and its effort mark. */
const MOCK_MODELS: Record<string, { model: string; effort: string | null }> = {
  default: { model: "Opus 4.7", effort: "xhigh" },
  opus: { model: "Opus 4.7", effort: "xhigh" },
  fable: { model: "Fable 5.1", effort: "high" },
  sonnet: { model: "Sonnet 4.5", effort: null },
  haiku: { model: "Haiku 4.5", effort: null },
};

type LineCb = (conn: string, msg: InMsg) => void;
type ExitCb = (conn: string, code: number | null, stderr: string) => void;
type LsCb = (ls: LsEntry[]) => void;
const lineCbs = new Set<LineCb>();
const exitCbs = new Set<ExitCb>();
const lsCbs = new Set<LsCb>();
const lsEndedCbs = new Set<(message: string) => void>();
const remoteConnCbs = new Set<(conn: string, holderId: string | null) => void>();
const closingCbs = new Set<(conn: string) => void>();
const conns = new Map<string, Conn>();
/** Holders whose Pane ended (`exit` with `close_pane`): gone from `ccrpc ls`. */
const closedPanes = new Set<string>();
/** Holders hidden or shown with `ccrpcSetPaneHidden` (over the recording's `hidden`). */
const paneHidden = new Map<string, boolean>();
let seq = 0;

/** What the E2E tests look at. */
export interface CcrpcProbe {
  sent: { conn: string; msg: Record<string, unknown> }[];
  /** `ccrpcWatchSessions(true)` calls (each starts a `ccrpc ls --watch` in the app). */
  watchStarts: number;
  /** A `ccrpc ls --watch` is running. */
  watching: boolean;
  guiLog: string[];
  badge: number;
  images: number;
  /** `ccrpcSetAnalyzer` calls. */
  analyzerSaves: number;
  /** `ccrpcOpenAnalyzerRecords` calls. */
  recordsOpened: number;
  /** `ccrpcSetPaneHidden` calls, in order. */
  paneHides: { holderId: string; hidden: boolean }[];
  /** 「新开的 Pane」 as last saved (`ccrpcSetHideNewPanes`). */
  hideNewPanes: boolean;
  /** Set by a test: what the directory picker returns (default `/tmp/mock-new`; null = cancelled). */
  pickDir?: string | null;
  /** Set by a test: the next Resume's `ready` names the Session already (the plugin checked in first). */
  resumeNamedInReady?: boolean;
  /** `ccrpcFork` calls, in order. */
  forks: { sessionId: string; cwd: string; at: string | null; restore: string | null; title: string }[];
  /** Set by a test: the next Fork fails (`fork-failed.jsonl`). */
  forkFails?: boolean;
  /** Set by a test: 「新建 Session」's claude takes this long after `ready` to start and name the Session. */
  newStartMs?: number;
  /** Recordings with `on_request` a test put into `ccrpc ls`, by name. */
  listed: string[];
  /** Puts a recording with `on_request` into `ccrpc ls` (pushed to the watch at once). */
  list: (name: string) => void;
  /** Sends `msg` on every open connection, as if ccrpc had written it (what no recording has). */
  emit: (msg: Record<string, unknown>) => void;
  /** The next command of this type is refused (after a moment, as ccrpc would answer). */
  failNext?: string;
  /** Set by a test: Claude Code runs a monitor of its own (not in `get_tasks`), so `exit` asks about it. */
  monitorRunning?: boolean;
  /** 「局域网访问」 as the mock Host has it (a test's change shows at the next refresh). */
  lan: LanStatus;
  /** Set by a test: the next `lanSetEnabled` is refused with this. */
  lanFails?: string;
  /** What `connectSnapshot` lists: the Host's connections, as `HostConn`s. */
  snapshot: Record<string, unknown>[];
  /** A client on the LAN starts a connection replaying the recording `name`; returns its id. */
  remoteConn: (name: string) => string;
  /** A client on the LAN Disconnects `conn`: the closing event, then its exit. */
  remoteDisconnect: (conn: string) => void;
}
const probe: CcrpcProbe = {
  sent: [],
  watchStarts: 0,
  watching: false,
  guiLog: [],
  badge: 0,
  images: 0,
  analyzerSaves: 0,
  recordsOpened: 0,
  paneHides: [],
  hideNewPanes: false,
  forks: [],
  listed: [],
  list: (name) => {
    probe.listed.push(name);
    later(pushLs);
  },
  emit: (msg) => {
    for (const id of conns.keys()) emit(id, msg as unknown as InMsg);
  },
  lan: { enabled: false, addresses: [], error: null },
  snapshot: [],
  remoteConn: (name) => {
    const fx = fixtures.find((f) => f.name === name);
    if (!fx) throw new Error(`没有 ${name} 的录制`);
    const id = open(fx);
    conns.get(id)!.remote = true;
    // After the replay's first events: the Host does not promise this comes first.
    later(() => remoteConnCbs.forEach((cb) => cb(id, fx.ls.holder_id)));
    return id;
  },
  remoteDisconnect: (id) => {
    const c = conns.get(id);
    if (!c || c.closed) return;
    c.closed = true;
    closingCbs.forEach((cb) => cb(id));
    later(() => exitCbs.forEach((cb) => cb(id, 0, "")));
  },
};

declare global {
  interface Window {
    /** E2E spy on the mock ccrpc. */
    __cvCcrpc?: CcrpcProbe;
    /** Set by an init script: what the probe starts with. */
    __cvCcrpcPreset?: Partial<CcrpcProbe>;
  }
}

const later = (f: () => void) => setTimeout(f, 0);

/** What `ccrpc ls --json` lists: the recordings that stand for a running Pane, as hidden or shown since. */
function lsNow(): LsEntry[] {
  return fixtures
    .filter(
      (f) =>
        f.name !== "new-session" &&
        !f.name.startsWith("resume-") &&
        f.name !== "resume" &&
        !f.name.startsWith("fork") &&
        (!f.onRequest || probe.listed.includes(f.name)) &&
        !closedPanes.has(f.ls.holder_id),
    )
    .map((f) => ({ ...f.ls, hidden: paneHidden.get(f.ls.holder_id) ?? f.ls.hidden }));
}

/** `ccrpc ls --watch` prints the list (on start and on every change). */
function pushLs() {
  if (!probe.watching) return;
  const ls = lsNow();
  for (const cb of lsCbs) cb(ls);
}

function emit(id: string, msg: InMsg) {
  const c = conns.get(id);
  if (!c || c.closed) return;
  if (msg.type === "state") c.state = msg;
  if (msg.type === "queue") c.queue = msg.items;
  if (msg.type === "session_info" || msg.type === "ready") {
    const { mode, model, effort, context_tokens } = msg as unknown as Record<string, unknown>;
    c.info = { mode, model, effort: effort ?? null, context_tokens };
  }
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
    if (msg.type === "ready" && c.startMs) setTimeout(step, c.startMs);
    else later(step);
  };
  later(step);
}

function respond(
  id: string,
  msg: Record<string, unknown>,
  data: Record<string, unknown> | null,
  error?: string,
  code = "mock",
) {
  const r: InMsg = {
    type: "response",
    id: msg.id as string,
    command: String(msg.type),
    success: error === undefined,
    ...(error === undefined ? { data: data ?? {} } : { error: { code, message: error } }),
  };
  later(() => emit(id, r));
}

function synth(id: string, c: Conn, msg: Record<string, unknown>) {
  switch (msg.type) {
    case "get_state": {
      const s = (c.state ?? { state: "idle" }) as Record<string, unknown>;
      return respond(id, msg, s);
    }
    case "get_tasks":
      return respond(id, msg, {
        tasks: c.fx.tasks.map((t) => (c.stopped.has(t.id) ? { ...t, status: "stopped" } : t)),
      });
    case "stop_task":
      c.stopped.add(String(msg.task_id));
      return respond(id, msg, {});
    // ↑ on the empty input box: what the person typed comes back (messages from another agent stay queued).
    case "edit_queue": {
      if ((c.state as { state?: string } | null)?.state !== "busy") return respond(id, msg, null, "not_busy");
      const all = c.queue ?? [];
      const items = all.filter(editable);
      if (items.length === 0) return respond(id, msg, null, "bad_request");
      later(() => emit(id, { type: "queue", items: all.filter((q) => !editable(q)) }));
      return respond(id, msg, { items });
    }
    // ctrl+enter: the TUI's queue goes out at once.
    case "send_now":
      later(() => emit(id, { type: "queue", items: [] }));
      return respond(id, msg, {});
    // `/model X`, `/effort X`: ccrpc says what was set at once (the command's output, the TUI's mark).
    case "set_model":
    case "set_effort": {
      if (c.state && (c.state as { state?: string }).state !== "idle") return respond(id, msg, null, "not_idle");
      const next =
        msg.type === "set_model"
          ? (MOCK_MODELS[String(msg.model)] ?? { model: String(msg.model), effort: null })
          : { model: c.info.model, effort: String(msg.level) };
      respond(id, msg, {});
      later(() => emit(id, { type: "session_info", ...c.info, ...next } as unknown as InMsg));
      return;
    }
    // Esc; like ccrpc, the state changes before the reply.
    case "abort":
      later(() => emit(id, { type: "state", state: "idle" } as InMsg));
      return respond(id, msg, { state: "idle" });
    // `/exit`; with `close_pane` the Pane ends too and leaves `ccrpc ls`. Background work still running makes Claude
    // Code ask first: without `stop_tasks` nothing ends.
    case "exit": {
      if (c.state && (c.state as { state?: string }).state !== "idle") return respond(id, msg, null, "not_idle");
      const running = probe.monitorRunning || c.fx.tasks.some((t) => t.status === "running" && !c.stopped.has(t.id));
      if (running && !msg.stop_tasks)
        return respond(id, msg, null, "还有后台任务在运行：加 stop_tasks 退出并停掉它们", "tasks_running");
      if (msg.close_pane) {
        closedPanes.add(c.fx.ls.holder_id);
        later(pushLs);
      }
      respond(id, msg, {});
      later(() => emit(id, { type: "state", state: "exited" } as InMsg));
      later(() => emit(id, { type: "session_exit" } as InMsg));
      return;
    }
    // Typed into the TUI; what it does shows on the screen, not in a reply. `/exit` ends claude (the Pane stays).
    case "slash":
      if (c.state && (c.state as { state?: string }).state !== "idle") return respond(id, msg, null, "not_idle");
      respond(id, msg, {});
      if (String(msg.command).replace(/^\//, "") === "exit") {
        later(() => emit(id, { type: "state", state: "exited" } as InMsg));
        later(() => emit(id, { type: "session_exit" } as InMsg));
      }
      return;
    case "answer_question":
      // Not in the recording (a question emitted by a test): answered.
      return respond(id, msg, { state: "busy" });
    default:
      return respond(id, msg, null, `录制里没有 ${String(msg.type)}`);
  }
}

function open(fx: Fixture, startMs?: number): string {
  const id = `mock${++seq}`;
  conns.set(id, { fx, cursor: 0, state: null, info: {}, closed: false, stopped: new Set(), startMs });
  replay(id);
  return id;
}

const subscribe = <T>(set: Set<T>, cb: T): Promise<Unlisten> => {
  set.add(cb);
  return Promise.resolve(() => void set.delete(cb));
};

type CcrpcApi = Pick<
  Api,
  | "ccrpcInfo"
  | "ccrpcSetAnalyzer"
  | "ccrpcTestAnalyzer"
  | "ccrpcAnalyzerRecords"
  | "ccrpcOpenAnalyzerRecords"
  | "ccrpcSetLaunchRules"
  | "ccrpcSetHideNewPanes"
  | "ccrpcSetPaneHidden"
  | "ccrpcWatchSessions"
  | "ccrpcConnect"
  | "ccrpcNew"
  | "ccrpcResume"
  | "ccrpcFork"
  | "ccrpcSend"
  | "ccrpcDisconnect"
  | "ccrpcGuiLog"
  | "ccrpcLogDir"
  | "ccrpcSaveImage"
  | "pickDir"
  | "setBadge"
  | "onCcrpcLine"
  | "onCcrpcExit"
  | "onCcrpcLs"
  | "onCcrpcLsEnded"
  | "connectSnapshot"
  | "onCcrpcRemoteConn"
  | "onCcrpcClosing"
  | "lanStatus"
  | "lanSetEnabled"
>;

/** The Session whose Resume fails in the mock: claude cannot find it (`resume-failed.jsonl`). */
export const RESUME_FAILS = "c04e77a2-0003-4000-8000-000000000003";

/** The launch rules saved in the mock settings (trimmed, rules without arguments dropped, like the backend). */
let launchRules: LaunchRule[] = [];

/** The analyzer as the mock app has it: a key from the login shell, nothing saved in the settings. */
const analyzer: { apiKey: string | null; baseUrl: string | null; model: string | null } = {
  apiKey: null,
  baseUrl: null,
  model: null,
};
const SHELL_KEY = "sk-shell-0000abcd";
function analyzerInfo(): AnalyzerInfo {
  const key = analyzer.apiKey ?? SHELL_KEY;
  return {
    enabled: true,
    keySource: analyzer.apiKey ? "settings" : "shell",
    keyHint: key.slice(-4),
    baseUrl: analyzer.baseUrl,
    model: analyzer.model,
    shellBaseUrl: null,
    shellModel: null,
  };
}

const ANALYSES_DIR = "/mock/.local/share/ccrpc/analyses";

/** Where the mock app listens with 「局域网访问」 on. */
const MOCK_LAN_ADDRESS = "192.168.2.50:38471";

export function createMockCcrpc(): CcrpcApi {
  window.__cvCcrpc = Object.assign(probe, window.__cvCcrpcPreset);
  return {
    ccrpcInfo: async () => ({
      ccrpcPath: "/mock/bin/ccrpc",
      doctor: [
        { level: "ok", text: "ok\ttoh\t/mock/bin/toh" },
        { level: "fail", text: "fail\tplugin\tccrpc 插件未启用" },
      ],
      analyzer: analyzerInfo(),
      launchRules,
      hideNewPanes: probe.hideNewPanes,
      logRoot: "/mock/Logs/dev.joy.codeck/ccrpc",
    }),
    ccrpcWatchSessions: async (on) => {
      probe.watching = on;
      if (on) {
        probe.watchStarts++;
        later(pushLs);
      }
    },
    ccrpcSetHideNewPanes: async (hide) => (probe.hideNewPanes = hide),
    ccrpcSetPaneHidden: async (holderId, hidden) => {
      probe.paneHides.push({ holderId, hidden });
      if (!fixtures.some((f) => f.ls.holder_id === holderId)) throw appError("internal", `没有会话 ${holderId}`);
      paneHidden.set(holderId, hidden);
      later(pushLs);
    },
    ccrpcSetAnalyzer: async (apiKey, baseUrl, model) => {
      probe.analyzerSaves++;
      const blank = (v: string | null) => (v?.trim() ? v.trim() : null);
      if (apiKey !== null) analyzer.apiKey = blank(apiKey);
      analyzer.baseUrl = blank(baseUrl);
      analyzer.model = blank(model);
      return analyzerInfo();
    },
    // A key ending in "bad" is refused like a wrong key; otherwise the test passes.
    ccrpcTestAnalyzer: async (apiKey, _baseUrl, model) => {
      const key = apiKey?.trim() || analyzer.apiKey || SHELL_KEY;
      if (key.endsWith("bad")) throw appError("internal", "分析器请求失败：http status: 401");
      return { model: model?.trim() || "deepseek-flash", elapsedMs: 2345 };
    },
    ccrpcAnalyzerRecords: async () => ({
      dir: ANALYSES_DIR,
      records: [
        {
          file: `${ANALYSES_DIR}/1793936310091-78130.json`,
          at: "2026-11-06T03:38:30Z",
          learned: true,
          title: "Computer Use wants to control these apps",
          error: null,
        },
        {
          file: `${ANALYSES_DIR}/1793891869922-60211.json`,
          at: "2026-11-05T15:17:49Z",
          learned: false,
          title: null,
          error: "分析器认为屏幕上没有等待作答的菜单",
        },
      ],
    }),
    ccrpcOpenAnalyzerRecords: async () => {
      probe.recordsOpened++;
      return null;
    },
    ccrpcSetLaunchRules: async (rules) => {
      launchRules = rules
        .map((r) => ({ dir: r.dir.trim().replace(/\/+$/, ""), args: r.args.trim() }))
        .filter((r) => r.args);
      return launchRules;
    },
    ccrpcConnect: async (holderId) => {
      const fx = fixtures.find((f) => f.ls.holder_id === holderId);
      if (!fx) throw appError("internal", `没有 ${holderId} 的录制`);
      const shared = [...conns].find(([, c]) => c.remote && !c.closed && c.fx === fx)?.[0];
      if (shared) return shared;
      const id = open(fx);
      // Tauri does not promise the command's answer before the connection's first events.
      if (fx.connectDelayMs) await new Promise((r) => setTimeout(r, fx.connectDelayMs));
      return id;
    },
    ccrpcResume: async (sessionId) => {
      const name = sessionId === RESUME_FAILS ? "resume-failed" : "resume";
      const fx = fixtures.find((f) => f.name === name);
      if (!fx) throw appError("internal", `没有 ${name} 的录制`);
      const inReady = !!probe.resumeNamedInReady;
      probe.resumeNamedInReady = false;
      const lines = fx.lines
        .filter((l) => !(inReady && l.msg?.type === "session_changed"))
        .map((l) =>
          l.msg?.type === "session_changed" || (inReady && l.msg?.type === "ready")
            ? { ...l, msg: { ...l.msg, session_id: sessionId } }
            : l,
        );
      return open({ ...fx, lines });
    },
    ccrpcFork: async (sessionId, cwd, at, restore, title) => {
      probe.forks.push({ sessionId, cwd, at, restore, title });
      const name = probe.forkFails
        ? "fork-failed"
        : at == null
          ? "fork-session"
          : restore === "both"
            ? "fork-warning"
            : "fork";
      probe.forkFails = false;
      const fx = fixtures.find((f) => f.name === name);
      if (!fx) throw appError("internal", `没有 ${name} 的录制`);
      // The Origin Session is the one asked for.
      const lines = fx.lines.map((l) =>
        l.msg?.type === "fork" && l.msg.origin ? { ...l, msg: { ...l.msg, origin: sessionId } } : l,
      );
      return open({ ...fx, lines });
    },
    ccrpcNew: async () => {
      const fx = fixtures.find((f) => f.name === "new-session");
      if (!fx) throw appError("internal", "没有 new-session 的录制");
      return open(fx, probe.newStartMs);
    },
    ccrpcSend: async (id, m) => {
      const msg = m as Record<string, unknown>;
      probe.sent.push({ conn: id, msg });
      const c = conns.get(id);
      if (!c || c.closed) throw appError("invalidQuery", "ccrpc 已退出");
      if (probe.failNext === msg.type) {
        probe.failNext = undefined;
        return void setTimeout(() => respond(id, msg, null, "mock 拒绝"), 300);
      }
      const next = c.fx.lines[c.cursor];
      if (next?.dir === "send" && next.msg?.type === msg.type) {
        c.cursor++;
        replay(id, { from: next.msg?.id, to: msg.id });
        return;
      }
      synth(id, c, msg);
    },
    ccrpcDisconnect: async (id) => {
      const c = conns.get(id);
      // Like the backend: a connection whose ccrpc has exited is unknown.
      if (!c || c.closed) throw appError("invalidQuery", "ccrpc 已退出");
      c.closed = true;
      later(() => exitCbs.forEach((cb) => cb(id, 0, "")));
    },
    ccrpcGuiLog: async (_conn, text) => {
      probe.guiLog.push(text);
    },
    ccrpcLogDir: async (id) => (conns.get(id)?.closed ? null : `/mock/Logs/dev.joy.codeck/ccrpc/${id}`),
    ccrpcSaveImage: async (_b64, ext) => `/mock/Caches/dev.joy.codeck/images/${++probe.images}.${ext}`,
    pickDir: async () => (probe.pickDir === undefined ? "/tmp/mock-new" : probe.pickDir),
    setBadge: async (n) => {
      probe.badge = n;
    },
    onCcrpcLine: (cb) => subscribe(lineCbs, cb),
    onCcrpcExit: (cb) => subscribe(exitCbs, cb),
    onCcrpcLs: (cb) => subscribe(lsCbs, cb),
    onCcrpcLsEnded: (cb) => subscribe(lsEndedCbs, cb),
    connectSnapshot: async () => JSON.stringify(probe.snapshot),
    onCcrpcRemoteConn: (cb) => subscribe(remoteConnCbs, cb),
    onCcrpcClosing: (cb) => subscribe(closingCbs, cb),
    lanStatus: async () => probe.lan,
    lanSetEnabled: async (on) => {
      if (probe.lanFails) {
        const why = probe.lanFails;
        probe.lanFails = undefined;
        throw appError("internal", why);
      }
      return (probe.lan = { enabled: on, addresses: on ? [MOCK_LAN_ADDRESS] : [], error: null });
    },
  };
}
