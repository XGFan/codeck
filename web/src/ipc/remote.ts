import type { InMsg, LsEntry } from "@/ipc/protocol/ccrpc";
import type { Api, Unlisten } from "./api";

declare global {
  interface Window {
    codeckPage?: {
      event?: (text: string) => void;
      setConnect(c: unknown): void;
    };
    webkit?: {
      messageHandlers?: {
        codeck?: { postMessage(msg: unknown): void };
        codeckRpc?: { postMessage(msg: unknown): Promise<unknown> };
      };
    };
  }
}

export type RemoteApi = Api & {
  /** Called on every (re)connect of the event socket (the daemon does not replay): refetch whatever is shown. */
  onResync(cb: () => void): Unlisten;
};

export type RpcReply = { result?: unknown; error?: unknown };
export type RpcSender = (method: string, args: Record<string, unknown>) => Promise<RpcReply>;

export interface RawEventSource {
  subscribe(onMessage: (text: string) => void): Unlisten;
}

export interface RemoteTransport {
  rpc: RpcSender;
  events: RawEventSource;
}

const BACKOFF_MAX_MS = 15_000;
const desktopOnly = (what: string) => Promise.reject({ code: "unsupported", message: `${what}仅桌面端支持` });
const unreachableReply = (): RpcReply => ({ error: { code: "http", message: "连不上 Host" } });

/** Direct HTTP (`POST /v1/rpc`) and one shared WebSocket (`/v1/events`), closed while the page is hidden. */
export function httpTransport(base: string): RemoteTransport {
  const root = base.replace(/\/+$/, "");

  async function rpc(method: string, args: Record<string, unknown>): Promise<RpcReply> {
    const res = await fetch(`${root}/v1/rpc`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ method, args }),
    });
    let body: RpcReply | null = null;
    try {
      body = (await res.json()) as RpcReply | null;
    } catch {
      // Not JSON: reported below by status.
    }
    if (!res.ok && !(body && "error" in body && body.error != null)) {
      return { error: { code: "http", message: `请求失败（${res.status}）` } };
    }
    return body ?? {};
  }

  const wsUrl = () => `${root.replace(/^http/, "ws")}/v1/events`;

  const events: RawEventSource = {
    subscribe(onMessage) {
      let active = true;
      let ws: WebSocket | null = null;
      let retry: ReturnType<typeof setTimeout> | null = null;
      let attempts = 0;

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
        if (!active || ws || document.visibilityState === "hidden") return;
        const s = new WebSocket(wsUrl());
        ws = s;
        s.onmessage = (e) => {
          const text = String(e.data);
          // Reset backoff once the daemon's handshake frame arrives.
          try {
            const frame = JSON.parse(text) as { event?: string } | null;
            if (frame && typeof frame === "object" && frame.event === "hello") attempts = 0;
          } catch {
            // Non-JSON frames are dropped by the consumer too.
          }
          onMessage(text);
        };
        s.onclose = () => {
          if (ws !== s) return;
          ws = null;
          if (!active || document.visibilityState === "hidden") return;
          const wait = Math.min(BACKOFF_MAX_MS, 500 * 2 ** attempts++);
          retry = setTimeout(open, wait);
        };
      }

      const onVisibility = () => {
        if (document.visibilityState === "hidden") close();
        else open();
      };

      document.addEventListener("visibilitychange", onVisibility);
      open();

      return () => {
        active = false;
        document.removeEventListener?.("visibilitychange", onVisibility);
        close();
      };
    },
  };

  return { rpc, events };
}

/** Relayed through the iOS app: RPC via `codeckRpc`, events pushed into `window.codeckPage.event`. */
export function nativeTransport(): RemoteTransport {
  const listeners = new Set<(text: string) => void>();

  // Attach on creation (module init in `ipc/index.ts`) without replacing `window.codeckPage`,
  // so `event` and `installNativeConnect`'s `setConnect` coexist regardless of order.
  if (typeof window !== "undefined") {
    const page = (window.codeckPage ??= {} as NonNullable<Window["codeckPage"]>);
    page.event = (text: string) => {
      listeners.forEach((cb) => cb(text));
    };
  }

  async function rpc(method: string, args: Record<string, unknown>): Promise<RpcReply> {
    const bridge = typeof window === "undefined" ? undefined : window.webkit?.messageHandlers?.codeckRpc;
    if (!bridge) return unreachableReply();
    try {
      const reply = await bridge.postMessage({ method, args });
      return reply && typeof reply === "object" ? (reply as RpcReply) : unreachableReply();
    } catch {
      return unreachableReply();
    }
  }

  const events: RawEventSource = {
    subscribe(onMessage) {
      listeners.add(onMessage);
      return () => {
        listeners.delete(onMessage);
      };
    },
  };

  return { rpc, events };
}

/** The daemon's command table over a remote transport (HTTP + WebSocket by default, or the iOS app bridge). */
export function createRemoteApi(target: string | RemoteTransport): RemoteApi {
  const transport = typeof target === "string" ? httpTransport(target) : target;

  async function rpc<T>(method: string, args: Record<string, unknown> = {}): Promise<T> {
    const body = await transport.rpc(method, args);
    if (body && "error" in body && body.error != null) throw body.error;
    return body?.result as T;
  }

  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  const resyncs = new Set<() => void>();
  let unsub: Unlisten | null = null;

  const wanted = () => handlers.size > 0 || resyncs.size > 0;

  function onMessage(text: string) {
    let msg: { event?: string; payload?: unknown } | null;
    try {
      msg = JSON.parse(text) as typeof msg;
    } catch {
      return;
    }
    if (!msg || typeof msg !== "object") return;
    if (msg.event === "hello") {
      resyncs.forEach((cb) => cb());
      return;
    }
    if (msg.event) handlers.get(msg.event)?.forEach((cb) => cb(msg.payload));
  }

  function sync() {
    if (wanted() && !unsub) {
      unsub = transport.events.subscribe(onMessage);
    } else if (!wanted() && unsub) {
      unsub();
      unsub = null;
    }
  }

  function listen<P>(event: string, cb: (payload: P) => void): Promise<Unlisten> {
    let set = handlers.get(event);
    if (!set) handlers.set(event, (set = new Set()));
    const h = cb as (payload: unknown) => void;
    set.add(h);
    sync();
    return Promise.resolve(() => {
      set.delete(h);
      if (!set.size) handlers.delete(event);
      sync();
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
    // Links open the desktop app, not this page.
    onOpenUrl: async () => () => {},
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
    keepAliveMinutes: () => desktopOnly("缓存保活"),
    setKeepAliveMinutes: () => desktopOnly("缓存保活"),
    onKeepAlive: async () => () => {},
    notify: async () => {},
    removeNotifications: async () => {},
    onNotificationClick: async () => () => {},
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
    connectSnapshot: () => desktopOnly("读取 Connect 状态"),
    onCcrpcRemoteConn: () => Promise.resolve(() => {}),
    onCcrpcClosing: () => Promise.resolve(() => {}),
    lanStatus: () => desktopOnly("局域网访问"),
    lanSetEnabled: () => desktopOnly("局域网访问"),
    lanPairStart: () => desktopOnly("局域网访问"),
    lanPairCancel: () => desktopOnly("局域网访问"),
    lanDevices: () => desktopOnly("局域网访问"),
    lanRemoveDevice: () => desktopOnly("局域网访问"),
    onLanDevices: () => Promise.resolve(() => {}),
    haaWatch: () => desktopOnly("haa Worker 状态"),
    onHaaWorkers: () => Promise.resolve(() => {}),
    onHaaWatchEnded: () => Promise.resolve(() => {}),
    getPlanUsage: () => desktopOnly("订阅额度"),
    onResync(cb) {
      resyncs.add(cb);
      sync();
      return () => {
        resyncs.delete(cb);
        sync();
      };
    },
  };
}

/** Remote API relayed through the iOS app (`codeckRpc` + `window.codeckPage.event`). */
export function createNativeRemoteApi(): RemoteApi {
  return createRemoteApi(nativeTransport());
}
