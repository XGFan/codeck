import { create } from "zustand";
import { api, type AppInfo } from "@/ipc";
import type { InMsg, LsEntry, MessagesPage, ResponseMsg, Task } from "@/protocol";
import { apply, applyPage, initial, type ConnState } from "./conn";

const FIRST_PAGE = 200;

interface Pending {
  conn: string;
  resolve: (r: ResponseMsg) => void;
}

interface Store {
  info: AppInfo | null;
  ls: LsEntry[];
  conns: Record<string, ConnState>;
  /** Holder ID of the Session on screen (a connection id while a new one has none yet). */
  selected: string | null;
  error: string | null;
  init(): () => void;
  select(key: string): Promise<void>;
  newSession(): Promise<void>;
  /** Sends a command and waits for its response (never rejects: failures are responses). */
  command(conn: string, msg: Record<string, unknown>): Promise<ResponseMsg>;
  loadOlder(conn: string): Promise<void>;
  reconnect(conn: string): Promise<void>;
  refreshTasks(conn: string): Promise<void>;
  dismissNotice(conn: string, id: number): void;
  setError(e: string | null): void;
}

const pending = new Map<string, Pending>();
/** Waiting for a connection's ccrpc to exit (reconnect: one ccrpc per Session at a time). */
const exitWaiters = new Map<string, () => void>();
/** Connections with an older page being fetched (one at a time each). */
const loadingOlder = new Set<string>();
let cmdSeq = 0;

/** The live connection to a Session (by Holder ID, or a connection id itself). */
export function connOf(s: { conns: Record<string, ConnState> }, key: string | null): ConnState | null {
  if (!key) return null;
  if (s.conns[key]) return s.conns[key];
  const all = Object.values(s.conns).filter((c) => c.holderId === key);
  return all.find((c) => !c.procExit) ?? all[all.length - 1] ?? null;
}

export const useStore = create<Store>((set, get) => {
  const update = (conn: string, f: (c: ConnState) => ConnState) =>
    set((s) => (s.conns[conn] ? { conns: { ...s.conns, [conn]: f(s.conns[conn]) } } : {}));

  const refreshState = async (conn: string) => {
    const r = await get().command(conn, { type: "get_state" });
    if (r.success && r.data) {
      const followUps = Array.isArray(r.data.follow_ups) ? (r.data.follow_ups as string[]) : [];
      update(conn, (c) => ({ ...c, followUps }));
    }
  };

  const firstPage = async (conn: string) => {
    const r = await get().command(conn, { type: "get_messages", limit: FIRST_PAGE, subagents: true });
    if (r.success && r.data) update(conn, (c) => applyPage(c, r.data as unknown as MessagesPage, false));
  };

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
    if (!before) return;
    update(conn, (c) => apply(c, msg));
    const after = get().conns[conn];
    switch (msg.type) {
      case "ready":
        // A new Session is shown under its Holder ID once it has one.
        if (get().selected === conn) set({ selected: msg.holder_id });
        void firstPage(conn);
        void refreshState(conn);
        void get().refreshTasks(conn);
        break;
      case "session_changed":
        void firstPage(conn);
        break;
      case "branch":
        // A branch before the loaded blocks emptied the stream: read it again.
        if (before.loaded && !after.loaded) void firstPage(conn);
        break;
      case "turn_end":
        if (get().selected !== after.holderId || document.hidden) update(conn, (c) => ({ ...c, unread: true }));
        void refreshState(conn);
        void get().refreshTasks(conn);
        break;
      case "state":
        if (before.state !== msg.state && (msg.state === "idle" || msg.state === "busy")) void refreshState(conn);
        break;
    }
  };

  const onExit = (conn: string, code: number | null, stderr: string) => {
    exitWaiters.get(conn)?.();
    exitWaiters.delete(conn);
    for (const [id, p] of pending) {
      if (p.conn !== conn) continue;
      pending.delete(id);
      p.resolve({ type: "response", id, command: "", success: false, error: { code: "exited", message: "ccrpc 已退出" } });
    }
    update(conn, (c) => ({ ...c, procExit: { code, stderr }, requests: [], progress: null }));
  };

  const addConn = (conn: string, holderId: string | null) =>
    set((s) => ({ conns: { ...s.conns, [conn]: initial(conn, holderId) } }));

  return {
    info: null,
    ls: [],
    conns: {},
    selected: null,
    error: null,

    init() {
      const offLine = api.onLine(onLine);
      const offExit = api.onExit(onExit);
      void api.appInfo().then((info) => set({ info }));
      const poll = async () => {
        try {
          set({ ls: await api.listSessions() });
        } catch (e) {
          set({ error: `列不出 Session：${String(e)}` });
        }
      };
      void poll();
      const t = window.setInterval(poll, 2000);
      return () => {
        offLine();
        offExit();
        window.clearInterval(t);
      };
    },

    async select(key) {
      set({ selected: key, error: null });
      const c = connOf(get(), key);
      if (c) {
        if (c.unread) update(c.conn, (x) => ({ ...x, unread: false }));
        return;
      }
      void api.guiLog(null, `connect ${key}`);
      try {
        const conn = await api.connect(key);
        addConn(conn, key);
      } catch (e) {
        set({ error: `连不上 ${key}：${String(e)}` });
      }
    },

    async newSession() {
      const dir = await api.pickDir();
      if (!dir) return;
      void api.guiLog(null, `new ${dir}`);
      try {
        const conn = await api.newSession(dir);
        addConn(conn, null);
        set({ selected: conn, error: null });
      } catch (e) {
        set({ error: `新建 Session 失败：${String(e)}` });
      }
    },

    command(conn, msg) {
      const id = `g${++cmdSeq}`;
      const full = { ...msg, id };
      void api.guiLog(conn, `command ${String(msg.type)} id=${id}`);
      return new Promise<ResponseMsg>((resolve) => {
        pending.set(id, {
          conn,
          resolve: (r) => {
            if (!r.success) void api.guiLog(conn, `failed ${String(msg.type)} id=${id} ${r.error?.code}: ${r.error?.message}`);
            resolve(r);
          },
        });
        api.send(conn, full).catch((e) => {
          pending.delete(id);
          resolve({ type: "response", id, command: String(msg.type), success: false, error: { code: "io", message: String(e) } });
        });
      });
    },

    async loadOlder(conn) {
      const c = get().conns[conn];
      if (!c || !c.more || c.first == null || loadingOlder.has(conn)) return;
      loadingOlder.add(conn);
      try {
        const r = await get().command(conn, { type: "get_messages", before: c.first, limit: FIRST_PAGE, subagents: true });
        if (r.success && r.data) update(conn, (x) => applyPage(x, r.data as unknown as MessagesPage, true));
      } finally {
        loadingOlder.delete(conn);
      }
    },

    async reconnect(conn) {
      const c = get().conns[conn];
      if (!c?.holderId) return;
      void api.guiLog(conn, `reconnect ${c.holderId}`);
      if (!c.procExit) {
        // The old ccrpc must be gone first: a second attach to the Session is refused.
        const gone = new Promise<void>((resolve) => {
          exitWaiters.set(conn, resolve);
          window.setTimeout(resolve, 5000);
        });
        await api.disconnect(conn).catch(() => {});
        await gone;
        exitWaiters.delete(conn);
      }
      set((s) => {
        const conns = { ...s.conns };
        delete conns[conn];
        return { conns };
      });
      try {
        const next = await api.connect(c.holderId);
        addConn(next, c.holderId);
      } catch (e) {
        set({ error: `重连失败：${String(e)}` });
      }
    },

    async refreshTasks(conn) {
      const r = await get().command(conn, { type: "get_tasks" });
      if (r.success && r.data && Array.isArray(r.data.tasks)) {
        const tasks = r.data.tasks as Task[];
        update(conn, (c) => ({ ...c, tasks }));
      }
    },

    dismissNotice(conn, id) {
      update(conn, (c) => ({ ...c, notices: c.notices.filter((n) => n.id !== id) }));
    },

    setError(e) {
      set({ error: e });
    },
  };
});

/** Sessions waiting for an answer: connected ones with a card, others by `ls` status. */
export function waitingKeys(s: { ls: LsEntry[]; conns: Record<string, ConnState> }): string[] {
  const out = new Set<string>();
  for (const e of s.ls) {
    const c = connOf(s, e.holder_id);
    if (c && !c.procExit ? c.requests.length > 0 || c.state === "waiting" : e.status === "waiting") out.add(e.holder_id);
  }
  return [...out];
}
