import { create } from "zustand";
import { api } from "@/ipc";
import type { DriveInfo, LiveEntry } from "@/ipc/bindings";
import { useUi } from "@/state/ui";
import { apply, initial, type ConnState } from "./conn";
import { createEarlyBuffer } from "./early";
import type { InMsg, LsEntry, ResponseMsg, Task } from "./protocol";

const POLL_MS = 2000;

/** A pasted image waiting to be sent: its saved path and a preview `data:` URL. */
export interface DraftImage {
  path: string;
  url: string;
}

/** What is typed in a Session's input box and not sent yet. */
export interface Draft {
  text: string;
  images: DraftImage[];
}

export const EMPTY_DRAFT: Draft = { text: "", images: [] };

interface DriveStore {
  /** `driveInfo`, asked for when first needed (its first call reads the login shell's environment). */
  info: DriveInfo | null;
  infoError: string | null;
  ls: LsEntry[];
  /** Why `ccrpc ls` stopped being polled. */
  lsError: string | null;
  conns: Record<string, ConnState>;
  /** The last failed action by Session id (a connection id before it has one): connect, set_mode, abort… */
  errors: Record<string, string>;
  /** Why the last 「新建 Session」 failed (shown in the Session list). */
  newError: string | null;
  /** The Viewer's Live Sessions from its `LiveChanged` events; null until the first one. */
  live: LiveEntry[] | null;
  /** Unsent input by Session id (a connection id before it has one): kept across switches and reconnects. */
  drafts: Record<string, Draft>;

  /** Subscribes to ccrpc's lines and exits and starts polling `ccrpc ls`; returns the cleanup. */
  start(): () => void;
  /** Whether `ccrpc ls` is worth polling: some Session is Live, or one is driven. Polled only while the window is visible. */
  setWanted(wanted: boolean): void;
  /** Polls `ccrpc ls` again after it stopped. */
  retry(): void;
  loadInfo(): Promise<void>;
  /** Attaches to the Holder of the Session `sessionId` (errors are shown on that Session). */
  connect(holderId: string, sessionId: string): Promise<void>;
  newSession(): Promise<void>;
  /** Sends a command and waits for its response (never rejects: failures are responses). */
  command(conn: string, msg: Record<string, unknown>): Promise<ResponseMsg>;
  disconnect(conn: string): Promise<void>;
  reconnect(conn: string): Promise<void>;
  refreshTasks(conn: string): Promise<void>;
  /** `get_state`'s queued follow-ups. */
  refreshState(conn: string): Promise<void>;
  setFollowUps(conn: string, followUps: string[]): void;
  dropRequest(conn: string, requestId: string): void;
  dismissNotice(conn: string, id: number): void;
  setError(key: string, e: string | null): void;
  setNewError(e: string | null): void;
  setDraft(key: string, f: (d: Draft) => Draft): void;
}

interface Pending {
  conn: string;
  resolve: (r: ResponseMsg) => void;
}

const pending = new Map<string, Pending>();
/** Waiting for a connection's ccrpc to exit (reconnect: one ccrpc per Session at a time). */
const exitWaiters = new Map<string, () => void>();
/** Connections being disconnected on purpose: their exit is not a failure. */
const closing = new Set<string>();
/** New Sessions to select once they have a session id. */
const selectOnReady = new Set<string>();
/** Events of connections whose id has not come back from `driveConnect` / `driveNew` yet. */
const early = createEarlyBuffer();
/** Connections already removed: their late events are dropped, not held. */
const removed = new Set<string>();
let cmdSeq = 0;
let pollTimer: number | undefined;
let polling = false;
/** Bumped whenever polling stops: a `ccrpc ls` answer from an older generation is dropped. */
let pollGen = 0;
let lastLs = "";
let wanted = false;

const errText = (e: unknown) => (e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e));

type Lookup = { ls: LsEntry[]; conns: Record<string, ConnState> };

/** The `ccrpc ls` entry of a Viewer Session (the Holder's session id is the Session id). */
export function lsEntryFor(s: Pick<Lookup, "ls">, sessionId: string | null): LsEntry | null {
  if (!sessionId) return null;
  return s.ls.find((e) => e.session_id === sessionId) ?? null;
}

/** The connection driving a Viewer Session: a live one first, else the last that exited. */
export function connFor(s: Lookup, sessionId: string | null): ConnState | null {
  if (!sessionId) return null;
  const holder = lsEntryFor(s, sessionId)?.holder_id;
  const all = Object.values(s.conns).filter((c) => (c.ready ? c.sessionId === sessionId : holder != null && c.holderId === holder));
  return all.find((c) => !c.procExit) ?? all[all.length - 1] ?? null;
}

/** A Holder already has a live connection (possibly to another Session after `session_changed`). */
export const holderConnected = (s: Pick<Lookup, "conns">, holderId: string) =>
  Object.values(s.conns).some((c) => c.holderId === holderId && !c.procExit);

export const draftKey = (c: ConnState) => c.sessionId ?? c.conn;
/** Where a connection's errors are shown: its Session (the connection before it has one). */
export const errorKey = draftKey;

export const useDrive = create<DriveStore>((set, get) => {
  const update = (conn: string, f: (c: ConnState) => ConnState) =>
    set((s) => (s.conns[conn] ? { conns: { ...s.conns, [conn]: f(s.conns[conn]) } } : {}));

  const select = (sessionId: string) => useUi.getState().selectSession(sessionId);

  const onLine = (conn: string, msg: InMsg) => {
    if (msg.type === "response") {
      const p = msg.id != null ? pending.get(String(msg.id)) : undefined;
      if (p) {
        pending.delete(String(msg.id));
        p.resolve(msg);
      }
      return;
    }
    const before = get().conns[conn];
    if (!before) {
      if (!removed.has(conn)) early.push(conn, { kind: "line", msg });
      return;
    }
    update(conn, (c) => apply(c, msg));
    const after = get().conns[conn]!;
    switch (msg.type) {
      case "ready":
        if (selectOnReady.has(conn) && msg.session_id) {
          selectOnReady.delete(conn);
          select(msg.session_id);
        }
        void get().refreshState(conn);
        void get().refreshTasks(conn);
        break;
      case "session_changed": {
        // `/clear` and the like: follow the Pane to its new Session if the old one is on screen.
        const open = useUi.getState().sessionId;
        if (selectOnReady.delete(conn) || (open != null && (open === msg.previous || open === before.sessionId))) select(msg.session_id);
        break;
      }
      case "turn_end":
        if (useUi.getState().sessionId !== after.sessionId || document.hidden) update(conn, (c) => ({ ...c, unread: true }));
        void get().refreshState(conn);
        void get().refreshTasks(conn);
        break;
      case "state":
        if (before.state !== msg.state && (msg.state === "idle" || msg.state === "busy")) void get().refreshState(conn);
        break;
    }
  };

  const onExit = (conn: string, code: number | null, stderr: string) => {
    if (!get().conns[conn]) {
      if (!removed.has(conn)) early.push(conn, { kind: "exit", code, stderr });
      return;
    }
    exitWaiters.get(conn)?.();
    exitWaiters.delete(conn);
    failPending(conn);
    selectOnReady.delete(conn);
    if (closing.delete(conn)) return removeConn(conn);
    update(conn, (c) => ({ ...c, procExit: { code, stderr }, requests: [], progress: null }));
  };

  /** Adds a connection, then replays what arrived for it before its id came back. */
  const addConn = (conn: string, holderId: string | null) => {
    set((s) => ({ conns: { ...s.conns, [conn]: initial(conn, holderId) } }));
    for (const e of early.take(conn)) {
      if (e.kind === "line") onLine(conn, e.msg);
      else onExit(conn, e.code, e.stderr);
    }
  };
  /** Commands still waiting on `conn` get a failure (its ccrpc exited or is no longer followed). */
  const failPending = (conn: string) => {
    for (const [id, p] of pending) {
      if (p.conn !== conn) continue;
      pending.delete(id);
      p.resolve({ type: "response", id, command: "", success: false, error: { code: "exited", message: "ccrpc 已退出" } });
    }
  };
  const removeConn = (conn: string) => {
    // Its exit may still come and is then dropped: nothing would answer what waits on it.
    failPending(conn);
    removed.add(conn);
    closing.delete(conn);
    exitWaiters.get(conn)?.();
    exitWaiters.delete(conn);
    set((s) => {
      const conns = { ...s.conns };
      delete conns[conn];
      return { conns };
    });
  };
  const setErr = (key: string, e: string | null) =>
    set((s) => {
      const errors = { ...s.errors };
      if (e == null) delete errors[key];
      else errors[key] = e;
      return { errors };
    });

  /** Polls while wanted, visible and not failed; when it starts again it asks at once. */
  const sync = () => {
    const go = wanted && document.visibilityState === "visible" && !get().lsError && !get().infoError;
    if (go && !polling) {
      polling = true;
      void poll(++pollGen);
    } else if (!go && polling) {
      stopPolling();
    }
  };
  const stopPolling = () => {
    polling = false;
    pollGen++;
    window.clearTimeout(pollTimer);
    pollTimer = undefined;
  };
  /** One `ccrpc ls`; the next is scheduled only after it answers. */
  const poll = async (gen: number) => {
    try {
      const ls = await api.driveList();
      if (gen !== pollGen) return;
      const text = JSON.stringify(ls);
      if (text !== lastLs) {
        lastLs = text;
        set({ ls });
      }
    } catch (e) {
      if (gen !== pollGen) return;
      set({ lsError: `ccrpc ls 失败：${errText(e)}` });
      return sync();
    }
    pollTimer = window.setTimeout(() => void poll(gen), POLL_MS);
  };

  /** The Session on screen has been seen. */
  const markSeen = () => {
    const c = connFor(get(), useUi.getState().sessionId);
    if (c?.unread) update(c.conn, (x) => ({ ...x, unread: false }));
  };

  return {
    info: null,
    infoError: null,
    ls: [],
    lsError: null,
    conns: {},
    errors: {},
    newError: null,
    live: null,
    drafts: {},

    start() {
      const offs: (() => void)[] = [];
      let gone = false;
      const keep = (p: Promise<() => void>) => void p.then((f) => (gone ? f() : offs.push(f)));
      // Fresh closures: a second `start` (StrictMode) must not share a listener the first one removes.
      keep(api.onDriveLine((conn, msg) => onLine(conn, msg)));
      keep(api.onDriveExit((conn, code, stderr) => onExit(conn, code, stderr)));
      keep(api.onLiveChanged((p) => set({ live: p.live })));
      const onVisible = () => {
        sync();
        if (document.visibilityState === "visible") markSeen();
      };
      document.addEventListener("visibilitychange", onVisible);
      const unUi = useUi.subscribe((s, p) => {
        if (s.sessionId !== p.sessionId) markSeen();
      });
      return () => {
        gone = true;
        offs.forEach((f) => f());
        document.removeEventListener("visibilitychange", onVisible);
        unUi();
        stopPolling();
      };
    },

    setWanted(w) {
      if (w === wanted) return sync();
      wanted = w;
      // Nothing Live and nothing driven: what `ccrpc ls` said last is stale.
      if (!w) {
        lastLs = "";
        set({ ls: [] });
      }
      sync();
    },

    retry() {
      set({ lsError: null, infoError: null });
      sync();
    },

    async loadInfo() {
      if (get().info) return;
      try {
        set({ info: await api.driveInfo(), infoError: null });
      } catch (e) {
        const why = `读取 ccrpc 信息失败：${errText(e)}`;
        set({ infoError: why });
        sync();
      }
    },

    async connect(holderId, sessionId) {
      setErr(sessionId, null);
      void api.driveGuiLog(null, `connect ${holderId}`);
      // A connection that exited is replaced.
      set((s) => ({ conns: Object.fromEntries(Object.entries(s.conns).filter(([, c]) => c.holderId !== holderId || !c.procExit)) }));
      try {
        addConn(await api.driveConnect(holderId), holderId);
      } catch (e) {
        setErr(sessionId, `连接失败：${errText(e)}`);
      }
    },

    async newSession() {
      const dir = await api.pickDir();
      if (!dir) return;
      set({ newError: null });
      void api.driveGuiLog(null, `new ${dir}`);
      try {
        const conn = await api.driveNew(dir);
        selectOnReady.add(conn);
        addConn(conn, null);
      } catch (e) {
        set({ newError: `新建 Session 失败：${errText(e)}` });
      }
    },

    command(conn, msg) {
      const id = `g${++cmdSeq}`;
      const full = { ...msg, id };
      void api.driveGuiLog(conn, `command ${String(msg.type)} id=${id}`);
      return new Promise<ResponseMsg>((resolve) => {
        pending.set(id, {
          conn,
          resolve: (r) => {
            if (!r.success) void api.driveGuiLog(conn, `failed ${String(msg.type)} id=${id} ${r.error?.code}: ${r.error?.message}`);
            resolve(r);
          },
        });
        api.driveSend(conn, full).catch((e: unknown) => {
          pending.delete(id);
          resolve({ type: "response", id, command: String(msg.type), success: false, error: { code: "io", message: errText(e) } });
        });
      });
    },

    async disconnect(conn) {
      const c = get().conns[conn];
      if (!c) return;
      void api.driveGuiLog(conn, "disconnect");
      if (c.procExit) return removeConn(conn);
      closing.add(conn);
      // The backend no longer knows it (its ccrpc already exited): nothing is left to wait for.
      await api.driveDisconnect(conn).catch(() => removeConn(conn));
    },

    async reconnect(conn) {
      const c = get().conns[conn];
      if (!c?.holderId) return;
      void api.driveGuiLog(conn, `reconnect ${c.holderId}`);
      if (!c.procExit) {
        // The old ccrpc must be gone first: a second attach to the Session is refused.
        const gone = new Promise<void>((resolve) => {
          exitWaiters.set(conn, resolve);
          window.setTimeout(resolve, 5000);
        });
        closing.add(conn);
        await api.driveDisconnect(conn).catch(() => removeConn(conn));
        await gone;
        exitWaiters.delete(conn);
      }
      removeConn(conn);
      try {
        addConn(await api.driveConnect(c.holderId), c.holderId);
      } catch (e) {
        setErr(errorKey(c), `重连失败：${errText(e)}`);
      }
    },

    async refreshTasks(conn) {
      const r = await get().command(conn, { type: "get_tasks" });
      if (r.success && r.data && Array.isArray(r.data.tasks)) {
        const tasks = r.data.tasks as Task[];
        update(conn, (c) => ({ ...c, tasks }));
      }
    },

    async refreshState(conn) {
      const r = await get().command(conn, { type: "get_state" });
      if (r.success && r.data) get().setFollowUps(conn, Array.isArray(r.data.follow_ups) ? (r.data.follow_ups as string[]) : []);
    },

    setFollowUps(conn, followUps) {
      update(conn, (c) => ({ ...c, followUps }));
    },

    dropRequest(conn, requestId) {
      update(conn, (c) => ({ ...c, requests: c.requests.filter((q) => q.request_id !== requestId) }));
    },

    dismissNotice(conn, id) {
      update(conn, (c) => ({ ...c, notices: c.notices.filter((n) => n.id !== id) }));
    },

    setError(key, e) {
      setErr(key, e);
    },

    setNewError(e) {
      set({ newError: e });
    },

    setDraft(key, f) {
      set((s) => ({ drafts: { ...s.drafts, [key]: f(s.drafts[key] ?? EMPTY_DRAFT) } }));
    },
  };
});

/** Sessions waiting for an answer: the Viewer's Live Sessions that wait, and connected Sessions with a card. */
export function waitingSessions(liveWaiting: string[], conns: Record<string, ConnState>): string[] {
  const out = new Set(liveWaiting);
  for (const c of Object.values(conns)) if (!c.procExit && c.requests.length > 0) out.add(c.sessionId ?? c.conn);
  return [...out];
}
