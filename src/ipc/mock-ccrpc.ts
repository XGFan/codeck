// Wire-replay ccrpc for the mock API (`pnpm dev:mock`, E2E): each `src/mocks/data/connect/*.jsonl` is a
// recorded `wire.jsonl` (`{t, dir: "send" | "recv", msg}`, plus `{dir: "exit", code, stderr}` and an
// optional first `{dir: "meta", ls: {...}}` that overrides the `ccrpc ls` entry) and shows up as one
// `ls` entry. A meta `session_id` (and `holder_id`) is also written into the recorded `ready`, so a
// recording can stand for a Session of CoDeck's mock data and keeps its ids when re-recorded. Connecting replays what ccrpc printed up to the first
// recorded command; each command sent that matches the next recorded one (by type) replays what
// followed it, with the response's id rewritten. Queries the recording does not have (get_state,
// get_tasks) are answered from what was replayed. `new-session` and `resume*` are not listed: `ccrpcNew` opens
// the first; `ccrpcResume` opens `resume` (its `session_changed` names the Session asked for), or
// `resume-failed` for the Session `RESUME_FAILS` (claude cannot find it).

import type { InMsg, LsEntry, Task } from "@/features/connect/protocol";
import type { Api, Unlisten } from "./api";
import type { AnalyzerInfo, AppError, LaunchRule } from "./bindings";

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
}

interface Fixture {
  name: string;
  lines: WireLine[];
  ls: LsEntry;
  connectDelayMs: number;
  tasks: Task[];
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
const lineCbs = new Set<LineCb>();
const exitCbs = new Set<ExitCb>();
const conns = new Map<string, Conn>();
/** Holders whose Pane ended (`exit` with `close_pane`): gone from `ccrpc ls`. */
const closedPanes = new Set<string>();
/** Holders hidden or shown with `ccrpcSetPaneHidden` (over the recording's `hidden`). */
const paneHidden = new Map<string, boolean>();
let seq = 0;

/** What the E2E tests look at. */
export interface CcrpcProbe {
  sent: { conn: string; msg: Record<string, unknown> }[];
  /** `ccrpcList` calls (each is a `ccrpc ls` process in the app). */
  lists: number;
  guiLog: string[];
  badge: number;
  images: number;
  /** `ccrpcSetAnalyzer` calls. */
  analyzerSaves: number;
  /** `ccrpcSetPaneHidden` calls, in order. */
  paneHides: { holderId: string; hidden: boolean }[];
  /** 「新开的 Pane」 as last saved (`ccrpcSetHideNewPanes`). */
  hideNewPanes: boolean;
  /** Set by a test: the next Resume's `ready` names the Session already (the plugin checked in first). */
  resumeNamedInReady?: boolean;
}
const probe: CcrpcProbe = {
  sent: [],
  lists: 0,
  guiLog: [],
  badge: 0,
  images: 0,
  analyzerSaves: 0,
  paneHides: [],
  hideNewPanes: false,
};

declare global {
  interface Window {
    /** E2E spy on the mock ccrpc. */
    __cvCcrpc?: CcrpcProbe;
  }
}

const later = (f: () => void) => setTimeout(f, 0);

function emit(id: string, msg: InMsg) {
  const c = conns.get(id);
  if (!c || c.closed) return;
  if (msg.type === "state") c.state = msg;
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
      return respond(id, msg, s);
    }
    case "get_tasks":
      return respond(id, msg, { tasks: c.fx.tasks });
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
    // `/exit`; with `close_pane` the Pane ends too and leaves `ccrpc ls`.
    case "exit":
      if (c.state && (c.state as { state?: string }).state !== "idle") return respond(id, msg, null, "not_idle");
      if (msg.close_pane) closedPanes.add(c.fx.ls.holder_id);
      respond(id, msg, {});
      later(() => emit(id, { type: "state", state: "exited" } as InMsg));
      later(() => emit(id, { type: "session_exit" } as InMsg));
      return;
    // Typed into the TUI; what it does shows on the screen, not in a reply. `/exit` ends claude (the Pane stays).
    case "slash":
      if (c.state && (c.state as { state?: string }).state !== "idle") return respond(id, msg, null, "not_idle");
      respond(id, msg, {});
      if (String(msg.command).replace(/^\//, "") === "exit") {
        later(() => emit(id, { type: "state", state: "exited" } as InMsg));
        later(() => emit(id, { type: "session_exit" } as InMsg));
      }
      return;
    default:
      return respond(id, msg, null, `录制里没有 ${String(msg.type)}`);
  }
}

function open(fx: Fixture): string {
  const id = `mock${++seq}`;
  conns.set(id, { fx, cursor: 0, state: null, info: {}, closed: false });
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
  | "ccrpcSetLaunchRules"
  | "ccrpcSetHideNewPanes"
  | "ccrpcSetPaneHidden"
  | "ccrpcList"
  | "ccrpcConnect"
  | "ccrpcNew"
  | "ccrpcResume"
  | "ccrpcSend"
  | "ccrpcDisconnect"
  | "ccrpcGuiLog"
  | "ccrpcLogDir"
  | "ccrpcSaveImage"
  | "pickDir"
  | "setBadge"
  | "onCcrpcLine"
  | "onCcrpcExit"
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

export function createMockCcrpc(): CcrpcApi {
  window.__cvCcrpc = probe;
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
    ccrpcList: async () => (
      probe.lists++,
      fixtures
        .filter(
          (f) =>
            f.name !== "new-session" &&
            !f.name.startsWith("resume-") &&
            f.name !== "resume" &&
            !closedPanes.has(f.ls.holder_id),
        )
        .map((f) => ({ ...f.ls, hidden: paneHidden.get(f.ls.holder_id) ?? f.ls.hidden }))
    ),
    ccrpcSetHideNewPanes: async (hide) => (probe.hideNewPanes = hide),
    ccrpcSetPaneHidden: async (holderId, hidden) => {
      probe.paneHides.push({ holderId, hidden });
      if (!fixtures.some((f) => f.ls.holder_id === holderId)) throw appError("internal", `没有会话 ${holderId}`);
      paneHidden.set(holderId, hidden);
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
    ccrpcSetLaunchRules: async (rules) => {
      launchRules = rules
        .map((r) => ({ dir: r.dir.trim().replace(/\/+$/, ""), args: r.args.trim() }))
        .filter((r) => r.args);
      return launchRules;
    },
    ccrpcConnect: async (holderId) => {
      const fx = fixtures.find((f) => f.ls.holder_id === holderId);
      if (!fx) throw appError("internal", `没有 ${holderId} 的录制`);
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
    ccrpcNew: async () => {
      const fx = fixtures.find((f) => f.name === "new-session");
      if (!fx) throw appError("internal", "没有 new-session 的录制");
      return open(fx);
    },
    ccrpcSend: async (id, m) => {
      const msg = m as Record<string, unknown>;
      probe.sent.push({ conn: id, msg });
      const c = conns.get(id);
      if (!c || c.closed) throw appError("invalidQuery", "ccrpc 已退出");
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
    pickDir: async () => "/tmp/mock-new",
    setBadge: async (n) => {
      probe.badge = n;
    },
    onCcrpcLine: (cb) => subscribe(lineCbs, cb),
    onCcrpcExit: (cb) => subscribe(exitCbs, cb),
  };
}
