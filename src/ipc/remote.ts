import type { InMsg, LsEntry } from "@/features/connect/protocol";
import type { Api, Unlisten } from "./api";

declare global {
  interface Window {
    webkit?: { messageHandlers?: { codeck?: { postMessage(msg: unknown): void } } };
  }
}

export type RemoteApi = Api & {
  /** Called on every (re)connect of the event socket (the daemon does not replay): refetch whatever is shown. */
  onResync(cb: () => void): Unlisten;
};

const BACKOFF_MAX_MS = 15_000;
const desktopOnly = (what: string) => Promise.reject({ code: "unsupported", message: `${what}仅桌面端支持` });

/** The daemon's command table over HTTP (`POST /v1/rpc`) and events over one WebSocket (`/v1/events`). */
export function createRemoteApi(base: string): RemoteApi {
  const root = base.replace(/\/+$/, "");

  async function rpc<T>(method: string, args: Record<string, unknown> = {}): Promise<T> {
    const res = await fetch(`${root}/v1/rpc`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ method, args }),
    });
    let body = null as { result?: T; error?: unknown } | null;
    try {
      body = (await res.json()) as typeof body;
    } catch {
      // Not JSON: reported below by status.
    }
    if (!res.ok || (body && "error" in body && body.error != null)) {
      throw body?.error ?? { code: "http", message: `请求失败（${res.status}）` };
    }
    return body?.result as T;
  }

  // --- events: one shared socket, closed while the page is hidden ---
  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  const resyncs = new Set<() => void>();
  let ws: WebSocket | null = null;
  let retry: ReturnType<typeof setTimeout> | null = null;
  let attempts = 0;

  const wsUrl = () => `${root.replace(/^http/, "ws")}/v1/events`;
  const wanted = () => handlers.size > 0 || resyncs.size > 0;

  function close() {
    if (retry) clearTimeout(retry);
    retry = null;
    const s = ws;
    ws = null;
    if (s) {
      s.onclose = null;
      s.close();
    }
  }

  function open() {
    if (ws || !wanted() || document.visibilityState === "hidden") return;
    const s = new WebSocket(wsUrl());
    ws = s;
    s.onmessage = (e) => {
      let msg: { event?: string; payload?: unknown };
      try {
        msg = JSON.parse(String(e.data)) as typeof msg;
      } catch {
        return;
      }
      if (msg.event === "hello") {
        attempts = 0;
        resyncs.forEach((cb) => cb());
        return;
      }
      if (msg.event) handlers.get(msg.event)?.forEach((cb) => cb(msg.payload));
    };
    s.onclose = () => {
      if (ws !== s) return;
      ws = null;
      if (!wanted() || document.visibilityState === "hidden") return;
      const wait = Math.min(BACKOFF_MAX_MS, 500 * 2 ** attempts++);
      retry = setTimeout(open, wait);
    };
  }

  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") close();
    else open();
  });

  function listen<P>(event: string, cb: (payload: P) => void): Promise<Unlisten> {
    let set = handlers.get(event);
    if (!set) handlers.set(event, (set = new Set()));
    const h = cb as (payload: unknown) => void;
    set.add(h);
    open();
    return Promise.resolve(() => {
      set.delete(h);
      if (!set.size) handlers.delete(event);
      if (!wanted()) close();
    });
  }

  return {
    getAppInfo: () => rpc("get_app_info"),
    setDataRoot: (path) => rpc("set_data_root", { path }),
    getIndexStatus: () => rpc("get_index_status"),
    rebuildIndex: () => rpc("rebuild_index"),
    listProjects: () => rpc("list_projects"),
    getSidebarProjects: () => rpc("get_sidebar_projects"),
    setSidebarProjects: (projects) => rpc("set_sidebar_projects", { projects }),
    listSessions: (query) => rpc("list_sessions", { query }),
    getSession: (sessionId) => rpc("get_session", { sessionId }),
    getTranscript: (req) => rpc("get_transcript", { req }),
    getToolDetail: (req) => rpc("get_tool_detail", { req }),
    getImage: (req) => rpc("get_image", { req }),
    search: (req) => rpc("search", { req }),
    resolveJump: (req) => rpc("resolve_jump", { req }),
    findInSession: (req) => rpc("find_in_session", { req }),
    revealSessionFile: () => desktopOnly("在 Finder 中显示"),
    syncSession: (sessionId) => rpc("sync_session", { sessionId }),
    getStats: (req) => rpc("get_stats", { req }),
    getDiagnostics: () => rpc("get_diagnostics"),
    listSlashCommands: (cwd) => rpc("list_slash_commands", { cwd }),
    completePaths: (cwd, partial, files) => rpc("complete_paths", { cwd, partial, files }),
    onIndexStatus: (cb) => listen("index-status-event", cb),
    onSessionsChanged: (cb) => listen("sessions-changed-event", cb),
    onLiveChanged: (cb) => listen("live-changed-event", cb),
    onPricesChanged: (cb) => listen("prices-changed-event", () => cb()),
    copyText: async (text) => {
      const native = window.webkit?.messageHandlers?.codeck;
      if (native) native.postMessage({ type: "copyText", text });
      else await navigator.clipboard.writeText(text);
    },
    setWindowTheme: async () => {},
    ccrpcInfo: () => rpc("ccrpc_info"),
    ccrpcSetAnalyzer: (apiKey, baseUrl, model) => rpc("ccrpc_set_analyzer", { apiKey, baseUrl, model }),
    ccrpcTestAnalyzer: (apiKey, baseUrl, model) => rpc("ccrpc_test_analyzer", { apiKey, baseUrl, model }),
    ccrpcAnalyzerRecords: () => rpc("ccrpc_analyzer_records"),
    ccrpcOpenAnalyzerRecords: () => desktopOnly("打开记录目录"),
    ccrpcSetLaunchRules: (rules) => rpc("ccrpc_set_launch_rules", { rules }),
    ccrpcSetHideNewPanes: (hide) => rpc("ccrpc_set_hide_new_panes", { hide }),
    ccrpcSetPaneHidden: async (holderId, hidden) => void (await rpc("ccrpc_set_pane_hidden", { holderId, hidden })),
    ccrpcWatchSessions: async (on) => void (await rpc("ccrpc_watch_sessions", { on })),
    ccrpcConnect: (holderId) => rpc("ccrpc_connect", { holderId }),
    ccrpcNew: (cwd) => rpc("ccrpc_new", { cwd }),
    ccrpcResume: (sessionId, cwd) => rpc("ccrpc_resume", { sessionId, cwd }),
    ccrpcFork: (sessionId, cwd, at, restore, title) => rpc("ccrpc_fork", { sessionId, cwd, at, restore, title }),
    ccrpcSend: async (conn, msg) => void (await rpc("ccrpc_send", { conn, line: JSON.stringify(msg) })),
    ccrpcDisconnect: async (conn) => void (await rpc("ccrpc_disconnect", { conn })),
    ccrpcGuiLog: async (conn, text) => void (await rpc("ccrpc_gui_log", { conn, text })),
    ccrpcLogDir: (conn) => rpc("ccrpc_log_dir", { conn }),
    ccrpcSaveImage: (base64, ext) => rpc("ccrpc_save_image", { base64, ext }),
    pickDir: () => desktopOnly("选择目录"),
    setBadge: async () => {},
    onCcrpcLine: (cb) =>
      listen<{ conn: string; line: string }>("ccrpc-line-event", ({ conn, line }) => {
        try {
          cb(conn, JSON.parse(line) as InMsg);
        } catch {
          // Not JSON: dropped, like the desktop.
        }
      }),
    onCcrpcExit: (cb) =>
      listen<{ conn: string; code: number | null; stderr: string }>("ccrpc-exit-event", (p) =>
        cb(p.conn, p.code, p.stderr),
      ),
    onCcrpcLs: (cb) =>
      listen<{ ls: string }>("ccrpc-ls-event", ({ ls }) => {
        try {
          cb(JSON.parse(ls) as LsEntry[]);
        } catch {
          // Not JSON: dropped.
        }
      }),
    onCcrpcLsEnded: (cb) => listen<{ message: string }>("ccrpc-ls-ended-event", (p) => cb(p.message)),
    onResync(cb) {
      resyncs.add(cb);
      open();
      return () => {
        resyncs.delete(cb);
        if (!wanted()) close();
      };
    },
  };
}
