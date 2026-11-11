import { create } from "zustand";
import { api } from "@/ipc";
import type { CcrpcInfo, LaunchRule, LiveEntry } from "@/ipc/bindings";
import { useUi } from "@/state/ui";
import { createCcrpcClient } from "./client";
import {
  apply,
  fromHost,
  initial,
  rewindAnswered,
  rewindShown,
  type ConnState,
  type HostConn,
  type PendingPrompt,
} from "./conn";
import { type Draft, EMPTY_DRAFT, loadDrafts, saveDrafts } from "./drafts";
import { addWithdrawn, loadWithdrawn, saveWithdrawn } from "./withdrawn";
import { createLsWatch } from "./lsWatch";
import { resumeFailure } from "./resume";
import { forkTitle } from "./fork";
import type { ForkEvent, InMsg, LsEntry, ResponseMsg, RewindOption, RewindRequest, Task } from "./protocol";
import { errorText, isNotFound } from "@/lib/errors";
import { ALL_SESSIONS } from "@/queries/hooks";
import { forgetSteers } from "./queue";

interface ConnectStore {
  /** `ccrpcInfo`, asked for when first needed (its first call reads the login shell's environment). */
  info: CcrpcInfo | null;
  infoError: string | null;
  ls: LsEntry[];
  /** Why `ccrpc ls` stopped being polled. */
  lsError: string | null;
  conns: Record<string, ConnState>;
  /** The last failed action by Session id (a connection id before it has one): connect, set_mode, abort… */
  errors: Record<string, string>;
  /** Why the last 「新建 Session」 failed (shown in the Session list). */
  newError: string | null;
  /** The CoDeck's Live Sessions from its `LiveChanged` events; null until the first one. */
  live: LiveEntry[] | null;
  /** Unsent input by Session id (a connection id before it has one): kept across switches and reconnects. */
  drafts: Record<string, Draft>;
  /** The prompts taken back (Esc before any reply), by Session: hidden in its transcript. */
  withdrawn: Record<string, string[]>;
  /** Sessions a Resume is under way for, until ccrpc names the Session or the failure is dealt with. */
  resuming: Record<string, true>;
  /** Sessions a Connect is under way for (自动 or from the header's menu), until it is attached or failed. */
  connecting: Record<string, true>;
  /**
   * Forks under way, by Origin Session (one at a time each): the prompt forked before (null: the whole Session) and the
   * Fork's Session once ccrpc names it (no placeholder row for it meanwhile).
   */
  forking: Record<string, { at: string | null; sessionId: string | null }>;

  /** Subscribes to ccrpc's lines and exits and starts polling `ccrpc ls`; returns the cleanup. */
  start(): () => void;
  /** Whether `ccrpc ls` is worth polling: some Session is Live, or one is connected. Polled only while the window is visible. */
  setWanted(wanted: boolean): void;
  /** Polls `ccrpc ls` again after it stopped. */
  retry(): void;
  loadInfo(): Promise<void>;
  /** Saves the analyzer's settings (see `ccrpcSetAnalyzer`); rejects with the reason. */
  saveAnalyzer(apiKey: string | null, baseUrl: string | null, model: string | null): Promise<void>;
  /** Saves the launch rules (see `ccrpcSetLaunchRules`); rejects with the reason. */
  saveLaunchRules(rules: LaunchRule[]): Promise<void>;
  /** Saves 「新开的 Pane」 (see `ccrpcSetHideNewPanes`); rejects with the reason. */
  saveHideNewPanes(hide: boolean): Promise<void>;
  /** Hides the Pane of the Session `sessionId` on the ToH Mac Client, or shows it (errors are shown on that Session). */
  setPaneHidden(holderId: string, sessionId: string, hidden: boolean): Promise<void>;
  /** Attaches to the Holder of the Session `sessionId` (errors are shown on that Session). */
  connect(holderId: string, sessionId: string): Promise<void>;
  /** 「新建 Session」 in `cwd` (`ccrpc new --cwd`), or in a directory picked now; selects it once ready. */
  newSession(cwd?: string): Promise<void>;
  /**
   * Resume: starts the ended Session `sessionId` again in a new Pane, from `cwd` (where it started), and connects;
   * a failure (claude exits before naming the Session, or 30 s without it) is shown on that Session and the Pane
   * closed.
   */
  resume(sessionId: string, cwd: string): Promise<void>;
  /**
   * Fork: a new Session copying the Main Line of `sessionId` up to before the prompt `at` (null: all of it), started
   * from `cwd` in a new Pane and Connected; `title` is the Origin Session's (the Fork's is made from it). On success
   * the Fork is selected (earlier when a card waits on it), with the prompt in its input box; a failure is shown on
   * the Origin Session.
   */
  fork(o: {
    sessionId: string;
    cwd: string;
    title: string;
    at: string | null;
    restore: "conversation" | "both" | null;
  }): Promise<void>;
  /** Sends a command and waits for its response (never rejects: failures are responses). */
  command(conn: string, msg: Record<string, unknown>): Promise<ResponseMsg>;
  disconnect(conn: string): Promise<void>;
  /**
   * 「关闭」: aborts a running turn, ends claude and its Pane, then disconnects. Background tasks are stopped only with
   * `stopTasks`; without it, Claude Code saying some still run ends nothing and resolves to `"tasks_running"`.
   */
  close(conn: string, stopTasks: boolean): Promise<"tasks_running" | void>;
  reconnect(conn: string): Promise<void>;
  refreshTasks(conn: string): Promise<void>;
  dropRequest(conn: string, requestId: string): void;
  /** 「回退」 on a prompt: `rewind` to its confirm page, shown as a card; a failure is shown on the Session. */
  rewind(conn: string, uuid: string): Promise<void>;
  /**
   * Answers the rewind's confirm page (`null`: cancel). Restoring the conversation puts the prompt into an empty
   * input box (a draft stays) and marks it in the transcript. A failure is shown on the Session.
   */
  answerRewind(conn: string, r: RewindRequest, option: RewindOption | null): Promise<void>;
  /** Enters or leaves 「选回退点」 (`/rewind`). */
  setPicking(conn: string, on: boolean): void;
  /** The rewound prompt left the indexed Main Line (the next prompt forked): the transcript shows it as usual. */
  clearRewound(conn: string): void;
  /** A prompt went out: the line about the kept draft goes (the marker stays until the new Branch is indexed). */
  rewoundSent(conn: string): void;
  /** The prompt taken back (`withdrawnText`) is this one in the transcript. */
  withdrawnFound(conn: string, id: string): void;
  /** The placeholder bubble of a prompt being sent (`uuid` once ccrpc answers), or none. */
  setPending(conn: string, p: PendingPrompt | null): void;
  dismissNotice(conn: string, id: number): void;
  setError(key: string, e: string | null): void;
  setNewError(e: string | null): void;
  setDraft(key: string, f: (d: Draft) => Draft): void;
}

/** How long a Resume may take to name its Session before it counts as failed. */
const RESUME_TIMEOUT_MS = 30_000;

type Lookup = { ls: LsEntry[]; conns: Record<string, ConnState> };

/** The `ccrpc ls` entry of a CoDeck Session (the Holder's session id is the Session id). */
export function lsEntryFor(s: Pick<Lookup, "ls">, sessionId: string | null): LsEntry | null {
  if (!sessionId) return null;
  return s.ls.find((e) => e.session_id === sessionId) ?? null;
}

/** A 「新建 Session」 connection ccrpc has not named the Session of yet: its page is selected by the connection's id. */
export const isNewPending = (c: ConnState) => c.newIn != null && c.sessionId == null;

/** A 「新建 Session」 connection before its first prompt: the Session has no transcript file yet. */
export const isNewUnprompted = (c: ConnState) => c.newIn != null && c.pending == null && c.lastTurn == null;

/** The connection to a CoDeck Session (or a new one's page): a live one first, else the last that exited. */
export function connFor(s: Lookup, sessionId: string | null): ConnState | null {
  if (!sessionId) return null;
  const pending = s.conns[sessionId];
  if (pending && isNewPending(pending)) return pending;
  const holder = lsEntryFor(s, sessionId)?.holder_id;
  const all = Object.values(s.conns).filter((c) =>
    c.forkOf === sessionId
      ? false
      : c.resumeOf === sessionId && c.sessionId == null
        ? true
        : c.ready
          ? c.sessionId === sessionId
          : holder != null && c.holderId === holder,
  );
  return all.find((c) => !c.procExit) ?? all[all.length - 1] ?? null;
}

/** A Holder already has a live connection (possibly to another Session after `session_changed`). */
export const holderConnected = (s: Pick<Lookup, "conns">, holderId: string) =>
  Object.values(s.conns).some((c) => c.holderId === holderId && isLive(c));

/**
 * A connection still on its Session. After `session_exit` ccrpc keeps running but is on nothing: the
 * Session may come back (`claude --resume` in the terminal) under a new process, which needs a new attach.
 */
export const isLive = (c: ConnState) => !c.procExit && !c.sessionExited;

export const draftKey = (c: ConnState) => c.sessionId ?? c.conn;
/** Where a connection's errors are shown: its Session (the connection before it has one). */
export const errorKey = draftKey;

export const useConnect = create<ConnectStore>((set, get) => {
  const client = createCcrpcClient(api);
  /** Connections being disconnected on purpose: their exit is not a failure. */
  const closing = new Set<string>();
  /** Resume connections being given up (one failure per connection). */
  const failingResumes = new Set<string>();
  const lsWatch = createLsWatch({
    watch: (on) => api.ccrpcWatchSessions(on),
    onLs: (ls) => set({ ls }),
    onError: (e) => set({ lsError: `ccrpc ls 失败：${errorText(e)}` }),
    // Watched only while the window is visible, and not after ccrpc failed.
    canWatch: () => document.visibilityState === "visible" && !get().lsError && !get().infoError,
  });

  const update = (conn: string, f: (c: ConnState) => ConnState) =>
    set((s) => (s.conns[conn] ? { conns: { ...s.conns, [conn]: f(s.conns[conn]) } } : {}));

  const select = (sessionId: string) => useUi.getState().selectSession(sessionId);

  const onLine = (conn: string, msg: InMsg) => {
    if (msg.type === "response") return client.answer(msg);
    const before = get().conns[conn];
    if (!before) return client.hold(conn, { kind: "line", msg });
    update(conn, (c) => apply(c, msg));
    const after = get().conns[conn]!;
    // A new Session is named: its page (open on the connection's id) and what was typed there carry on as it.
    if (isNewPending(before) && after.sessionId) named(conn, after.sessionId);
    // A Resume is through once its Session is named: in `ready` (checked in already) or a later `session_changed`.
    if (before.resumeOf && !before.sessionId && after.sessionId) setResuming(before.resumeOf, false);
    if (before.forkOf) {
      const origin = before.forkOf;
      if (msg.type === "fork") return forked(conn, origin, msg);
      if (after.sessionId !== before.sessionId)
        setForking(origin, { at: get().forking[origin]?.at ?? null, sessionId: after.sessionId });
      // A card waits on the Fork before it is done (the rewind's ⚠ confirm page): shown on its Session.
      const shown = !!before.sessionId && before.requests.length > 0;
      if (after.sessionId && after.requests.length > 0 && !shown && useUi.getState().sessionId === origin)
        select(after.sessionId);
    }
    switch (msg.type) {
      case "ready":
        // A new Session's input box takes typing from now on (its page was opened for that).
        if (after.newIn != null && [conn, after.sessionId].includes(useUi.getState().sessionId))
          window.setTimeout(() => focusInput(conn), 0);
        void get().refreshTasks(conn);
        break;
      case "session_changed": {
        // `/clear` and the like: follow the Pane to its new Session if the old one is on screen.
        const open = useUi.getState().sessionId;
        if (open != null && (open === msg.previous || open === before.sessionId)) select(msg.session_id);
        break;
      }
      case "session_exit":
        // claude exited before naming the Session: a Resume that did not start (e.g. it cannot find the Session).
        if (before.resumeOf && !before.sessionId) void failResume(conn, null);
        break;
      case "turn_end": {
        if (useUi.getState().sessionId !== after.sessionId || document.hidden)
          update(conn, (c) => ({ ...c, unread: true }));
        // Taken back before any reply: as in the terminal, the prompt goes back into the input box (an empty one).
        const back = msg.aborted ? msg.restored_text : null;
        const dk = draftKey(after);
        const draft = get().drafts[dk] ?? EMPTY_DRAFT;
        if (back && !draft.text.trim() && draft.images.length === 0) get().setDraft(dk, (d) => ({ ...d, text: back }));
        void get().refreshTasks(conn);
        break;
      }
    }
  };

  const onExit = (conn: string, code: number | null, stderr: string) => {
    if (!get().conns[conn]) return client.hold(conn, { kind: "exit", code, stderr });
    client.exited(conn);
    client.failPending(conn);
    const c = get().conns[conn];
    // ccrpc itself ended before naming the Session: that Resume is over (its card says why).
    if (c?.resumeOf && !c.sessionId && !failingResumes.has(conn)) setResuming(c.resumeOf, false);
    // ccrpc ended before its `fork` event: that Fork failed, nothing of it is kept.
    if (c?.forkOf) {
      if (!closing.has(conn))
        failFork(
          conn,
          c.forkOf,
          lastLine(stderr).replace(/^ccrpc: /, "") ||
            (code == null ? "ccrpc 被信号终止" : `ccrpc 退出了（退出码 ${code}）`),
        );
      setForking(c.forkOf, null);
      closing.add(conn);
    }
    if (closing.delete(conn)) return removeConn(conn);
    update(conn, (c) => ({
      ...c,
      procExit: { code, stderr },
      requests: [],
      progress: null,
      picking: false,
      rewinding: null,
      rewound: null,
    }));
  };

  /**
   * A new Session got its id: the draft typed on its page moves to the Session, and the page, if still open, is the
   * Session's from now on (opened elsewhere meanwhile, the selection stays).
   */
  const named = (conn: string, sessionId: string) => {
    const draft = get().drafts[conn];
    if (draft) {
      set((s) => {
        const drafts = { ...s.drafts, [sessionId]: draft };
        delete drafts[conn];
        return { drafts };
      });
      saveDrafts(get().drafts);
    }
    if (useUi.getState().sessionId === conn) select(sessionId);
  };

  const replayEarly = (conn: string) => {
    for (const e of client.take(conn)) {
      if (e.kind === "line") onLine(conn, e.msg);
      else onExit(conn, e.code, e.stderr);
    }
  };
  /**
   * Adds a connection, then replays what arrived for it before its id came back. One already here keeps its state: the
   * Host hands every client the same connection.
   */
  const addConn = (
    conn: string,
    holderId: string | null,
    resumeOf: string | null = null,
    forkOf: string | null = null,
    newIn: string | null = null,
  ) => {
    if (!get().conns[conn])
      set((s) => ({ conns: { ...s.conns, [conn]: initial(conn, holderId, resumeOf, forkOf, newIn) } }));
    replayEarly(conn);
  };
  const removeConn = (conn: string) => {
    // Its exit may still come and is then dropped: nothing would answer what waits on it.
    client.forget(conn);
    closing.delete(conn);
    forgetSteers(conn);
    set((s) => {
      const conns = { ...s.conns };
      delete conns[conn];
      return { conns };
    });
  };
  /** Closes a connection's ccrpc (if still running), waits for it to exit (≤5 s), and drops the connection. */
  const closeAndWait = async (conn: string) => {
    const c = get().conns[conn];
    void api.ccrpcGuiLog(conn, "close (replaced)");
    if (c && !c.procExit) {
      const gone = client.waitExit(conn, 5000);
      closing.add(conn);
      await api.ccrpcDisconnect(conn).catch(() => removeConn(conn));
      await gone;
    }
    removeConn(conn);
  };
  /**
   * Gives a Resume up: the reason (claude's last line on the Pane, or `why`) goes on its Session, then the new Pane is
   * closed and the connection dropped.
   */
  const failResume = async (conn: string, timedOut: string | null) => {
    const c = get().conns[conn];
    if (!c?.resumeOf || failingResumes.has(conn)) return;
    const sessionId = c.resumeOf;
    failingResumes.add(conn);
    // Let the shell draw its prompt again, so claude's last line is above the cursor.
    await new Promise((r) => window.setTimeout(r, 300));
    const r = await get().command(conn, { type: "get_screen" });
    const lines = r.success && Array.isArray(r.data?.lines) ? (r.data.lines as string[]) : [];
    const cursor = r.success && Array.isArray(r.data?.cursor) ? Number((r.data.cursor as unknown[])[0]) : null;
    const line = resumeFailure(lines, cursor);
    let reason = timedOut ? `${timedOut}（${line}）` : line;
    // `exit` needs claude idle or gone: one still on a screen of its own keeps its Pane, say so.
    const e = await get().command(conn, { type: "exit", close_pane: true, stop_tasks: true });
    if (!e.success) reason += `；Pane 没有关掉：${e.error?.message ?? ""}`;
    void api.ccrpcGuiLog(conn, `resume failed: ${reason}`);
    setErr(sessionId, `Resume 失败：${reason}`);
    await closeAndWait(conn);
    failingResumes.delete(conn);
    setResuming(sessionId, false);
  };
  const setResuming = (sessionId: string, on: boolean) =>
    set((s) => {
      if (!!s.resuming[sessionId] === on) return {};
      const resuming = { ...s.resuming };
      if (on) resuming[sessionId] = true;
      else delete resuming[sessionId];
      return { resuming };
    });
  const setForking = (origin: string, f: ConnectStore["forking"][string] | null) =>
    set((s) => {
      const forking = { ...s.forking };
      if (f) forking[origin] = f;
      else delete forking[origin];
      return { forking };
    });
  /** A Fork failed: said on its Origin Session, shown again if its card had taken the Fork's place (ccrpc closes the Pane). */
  const failFork = (conn: string, origin: string, why: string) => {
    const c = get().conns[conn];
    if (c?.sessionId && useUi.getState().sessionId === c.sessionId) select(origin);
    void api.ccrpcGuiLog(conn, `fork failed: ${why}`);
    setErr(origin, `Fork 失败：${why}`);
  };
  /** The `fork` event: the Fork is selected (from its Origin) with the prompt in its input box, or it failed. */
  const forked = (conn: string, origin: string, msg: ForkEvent) => {
    setForking(origin, null);
    if (msg.error) {
      // ccrpc closes claude and the Pane, then exits: that exit only drops the connection.
      closing.add(conn);
      return failFork(conn, origin, msg.error.message);
    }
    const id = msg.session_id;
    void api.ccrpcGuiLog(conn, `forked ${origin} → ${id}`);
    const draft = get().drafts[id] ?? EMPTY_DRAFT;
    if (msg.text && !draft.text.trim() && draft.images.length === 0)
      get().setDraft(id, (d) => ({ ...d, text: msg.text! }));
    if (msg.title_error) setErr(id, `Fork 的标题没有改成：${msg.title_error}`);
    const open = useUi.getState().sessionId;
    if (open !== origin && open !== id) return;
    select(id);
    focusInput(conn);
  };
  const attach = async (holderId: string, sessionId: string) => {
    setErr(sessionId, null);
    void api.ccrpcGuiLog(null, `connect ${holderId}`);
    // From here a connection `ccrpcConnect` returns and this window does not know is a new one: its `ready` is to come.
    await snapshot;
    // Connections on nothing any more (ccrpc exited, or the Session ended and was resumed since) are
    // replaced; a ccrpc still attached must be gone first, a second attach to the Session is refused.
    const stale = Object.values(get().conns).filter(
      (c) => (c.holderId === holderId || c.sessionId === sessionId) && !isLive(c),
    );
    await Promise.all(stale.map((c) => closeAndWait(c.conn)));
    try {
      addConn(await api.ccrpcConnect(holderId), holderId);
    } catch (e) {
      setErr(sessionId, `连接失败：${errorText(e)}`);
    }
  };
  const setErr = (key: string, e: string | null) =>
    set((s) => {
      const errors = { ...s.errors };
      if (e == null) delete errors[key];
      else errors[key] = e;
      return { errors };
    });

  /** The Session on screen has been seen. */
  const markSeen = () => {
    const c = connFor(get(), useUi.getState().sessionId);
    if (c?.unread) update(c.conn, (x) => ({ ...x, unread: false }));
  };

  /** Another Session is on screen: picking a prompt ends with the one left (the rewound marker stays with it). */
  const endPicking = () => {
    const open = connFor(get(), useUi.getState().sessionId)?.conn;
    for (const c of Object.values(get().conns))
      if (c.conn !== open && c.picking) update(c.conn, (x) => ({ ...x, picking: false }));
  };
  /** Rewinds being answered (`conn|request_id`): Esc, 「中止」 and the card's buttons send one answer. */
  const answering = new Set<string>();

  /** The Host's connections being read (`start`); settled before a Connect. */
  let snapshot: Promise<void> = Promise.resolve();
  /** While they are: what a client on the LAN did, for after them (it may be about a connection they bring). */
  let afterSnapshot: (() => void)[] | null = null;
  const onceSnapshotted = (f: () => void) => void (afterSnapshot ? afterSnapshot.push(f) : f());

  /** A client on the LAN started or shared this connection: this window shows it too. */
  const onRemoteConn = (conn: string, holderId: string | null) => onceSnapshotted(() => addConn(conn, holderId));

  /** A client on the LAN is ending this connection: its exit is not a failure. */
  const onClosing = (conn: string) =>
    onceSnapshotted(() => {
      const c = get().conns[conn];
      if (!c) return;
      if (c.procExit) removeConn(conn);
      else closing.add(conn);
    });

  /**
   * Brings in the Host's connections this window does not know (a Reload lost them, and `ready` comes only once). Waits
   * for the subscriptions, so whatever happens after the Host's reading is held for the connection and replayed. Only
   * the `start` still running reads: StrictMode's first one is gone by then.
   */
  const readSnapshot = async (subscribed: Promise<unknown>[], isGone: () => boolean) => {
    const held: (() => void)[] = [];
    afterSnapshot = held;
    try {
      await Promise.allSettled(subscribed);
      if (isGone()) return;
      const all = JSON.parse(await api.connectSnapshot()) as HostConn[];
      if (isGone()) return;
      for (const h of all) {
        if (h.disconnected || get().conns[h.conn]) continue;
        set((s) => ({ conns: { ...s.conns, [h.conn]: fromHost(h) } }));
        replayEarly(h.conn);
      }
    } catch (e) {
      void api.ccrpcGuiLog(null, `connect snapshot failed: ${errorText(e)}`);
    } finally {
      if (afterSnapshot === held) {
        afterSnapshot = null;
        held.forEach((f) => f());
      }
    }
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
    drafts: loadDrafts(),
    withdrawn: loadWithdrawn(),
    resuming: {},
    connecting: {},
    forking: {},

    start() {
      const offs: (() => void)[] = [];
      let gone = false;
      const subscribed: Promise<unknown>[] = [];
      const keep = (p: Promise<() => void>) => void subscribed.push(p.then((f) => (gone ? f() : offs.push(f))));
      // Fresh closures: a second `start` (StrictMode) must not share a listener the first one removes.
      keep(api.onCcrpcLine((conn, msg) => onLine(conn, msg)));
      keep(api.onCcrpcExit((conn, code, stderr) => onExit(conn, code, stderr)));
      keep(api.onCcrpcRemoteConn((conn, holderId) => onRemoteConn(conn, holderId)));
      keep(api.onCcrpcClosing((conn) => onClosing(conn)));
      keep(api.onLiveChanged((p) => set({ live: p.live })));
      keep(api.onCcrpcLs((ls) => lsWatch.list(ls)));
      keep(api.onCcrpcLsEnded((message) => lsWatch.ended(message)));
      snapshot = readSnapshot(subscribed, () => gone);
      const onVisible = () => {
        lsWatch.sync();
        if (document.visibilityState === "visible") markSeen();
      };
      document.addEventListener("visibilitychange", onVisible);
      const unUi = useUi.subscribe((s, p) => {
        if (s.sessionId === p.sessionId) return;
        markSeen();
        endPicking();
      });
      return () => {
        gone = true;
        offs.forEach((f) => f());
        document.removeEventListener("visibilitychange", onVisible);
        unUi();
        lsWatch.stop();
      };
    },

    setWanted(w) {
      // Nothing Live and nothing connected: what `ccrpc ls` said last is stale.
      lsWatch.setWanted(w);
    },

    retry() {
      set({ lsError: null, infoError: null });
      lsWatch.sync();
    },

    async saveAnalyzer(apiKey, baseUrl, model) {
      const analyzer = await api.ccrpcSetAnalyzer(apiKey, baseUrl, model);
      set((s) => (s.info ? { info: { ...s.info, analyzer } } : {}));
    },

    async saveLaunchRules(rules) {
      const launchRules = await api.ccrpcSetLaunchRules(rules);
      set((s) => (s.info ? { info: { ...s.info, launchRules } } : {}));
    },

    async saveHideNewPanes(hide) {
      const hideNewPanes = await api.ccrpcSetHideNewPanes(hide);
      set((s) => (s.info ? { info: { ...s.info, hideNewPanes } } : {}));
    },

    async setPaneHidden(holderId, sessionId, hidden) {
      setErr(sessionId, null);
      try {
        await api.ccrpcSetPaneHidden(holderId, hidden);
        // Shown at once; the watch reports it too.
        set((s) => ({ ls: s.ls.map((e) => (e.holder_id === holderId ? { ...e, hidden } : e)) }));
      } catch (e) {
        setErr(sessionId, `${hidden ? "隐藏" : "显示"} Pane 失败：${errorText(e)}`);
      }
    },

    async loadInfo() {
      if (get().info) return;
      try {
        set({ info: await api.ccrpcInfo(), infoError: null });
      } catch (e) {
        const why = `读取 ccrpc 信息失败：${errorText(e)}`;
        set({ infoError: why });
        lsWatch.sync();
      }
    },

    async connect(holderId, sessionId) {
      if (get().connecting[sessionId]) return;
      set((s) => ({ connecting: { ...s.connecting, [sessionId]: true } }));
      try {
        await attach(holderId, sessionId);
      } finally {
        set((s) => {
          const connecting = { ...s.connecting };
          delete connecting[sessionId];
          return { connecting };
        });
      }
    },

    async newSession(cwd) {
      const dir = cwd ?? (await api.pickDir());
      if (!dir) return;
      set({ newError: null });
      void api.ccrpcGuiLog(null, `new ${dir}`);
      try {
        const conn = await api.ccrpcNew(dir);
        // Its page opens now, on the connection's id; claude names the Session in a second or two.
        select(conn);
        addConn(conn, null, null, null, dir);
      } catch (e) {
        set({ newError: `新建 Session 失败：${errorText(e)}` });
      }
    },

    async resume(sessionId, cwd) {
      // One at a time per Session (the header and the ended card both offer it).
      if (get().resuming[sessionId]) return;
      setResuming(sessionId, true);
      setErr(sessionId, null);
      void api.ccrpcGuiLog(null, `resume ${sessionId}`);
      // An ended connection on this Session (its ccrpc may still run, driving nothing) goes first.
      const stale = Object.values(get().conns).filter(
        (c) => (c.sessionId === sessionId || c.resumeOf === sessionId) && !isLive(c),
      );
      await Promise.all(stale.map((c) => closeAndWait(c.conn)));
      // claude continues from the end of the Main Line: show that, not a Branch picked earlier.
      if (useUi.getState().sessionId === sessionId) useUi.setState({ branchChoices: [] });
      try {
        const conn = await api.ccrpcResume(sessionId, cwd);
        addConn(conn, null, sessionId);
        // Not while a card waits for you (the folder trust dialog comes before claude names the Session).
        const check = () => {
          const c = get().conns[conn];
          // Gone: unless a later attempt for this Session is on its way.
          if (!c) {
            const other = Object.values(get().conns).some((x) => x.resumeOf === sessionId && !x.sessionId && isLive(x));
            return other ? undefined : setResuming(sessionId, false);
          }
          if (c.sessionId || !isLive(c)) return;
          if (c.requests.length > 0) window.setTimeout(check, RESUME_TIMEOUT_MS);
          else void failResume(conn, "30 秒内没有启动");
        };
        window.setTimeout(check, RESUME_TIMEOUT_MS);
      } catch (e) {
        setErr(sessionId, `Resume 失败：${errorText(e)}`);
        setResuming(sessionId, false);
      }
    },

    async fork({ sessionId, cwd, title, at, restore }) {
      if (get().forking[sessionId]) return;
      setForking(sessionId, { at, sessionId: null });
      // The Origin may be Connected: only an earlier Fork's failure goes.
      if (get().errors[sessionId]?.startsWith("Fork 失败")) setErr(sessionId, null);
      void api.ccrpcGuiLog(null, `fork ${sessionId}${at ? ` at ${at}` : ""}${restore ? ` ${restore}` : ""}`);
      try {
        // Only to number the title: without the list it is just ` (Fork)`.
        const taken = await api
          .listSessions(ALL_SESSIONS)
          .then((all) => all.map((s) => s.title))
          .catch(() => []);
        const conn = await api.ccrpcFork(sessionId, cwd, at, restore, forkTitle(title, taken));
        addConn(conn, null, null, sessionId);
      } catch (e) {
        setErr(sessionId, `Fork 失败：${errorText(e)}`);
        setForking(sessionId, null);
      }
    },

    command(conn, msg) {
      return client.command(conn, msg);
    },

    async disconnect(conn) {
      const c = get().conns[conn];
      if (!c) return;
      void api.ccrpcGuiLog(conn, "disconnect");
      if (c.procExit) return removeConn(conn);
      closing.add(conn);
      // The backend no longer knows it (its ccrpc already exited): nothing is left to wait for.
      await api.ccrpcDisconnect(conn).catch(() => removeConn(conn));
    },

    async close(conn, stopTasks) {
      const c = get().conns[conn];
      if (!c || !isLive(c)) return;
      void api.ccrpcGuiLog(conn, "close");
      const fail = (r: ResponseMsg) => {
        closing.delete(conn);
        setErr(errorKey(c), `关闭失败：${r.error?.message ?? r.error?.code ?? ""}`);
      };
      if (c.state === "busy" || c.state === "waiting") {
        const r = await get().command(conn, { type: "abort" });
        if (!r.success) return fail(r);
      }
      // Its ccrpc may exit once the Pane is gone: that exit is not a failure.
      closing.add(conn);
      const r = await get().command(conn, { type: "exit", close_pane: true, stop_tasks: stopTasks });
      if (r.error?.code === "tasks_running") {
        closing.delete(conn);
        return "tasks_running";
      }
      if (!r.success) return fail(r);
      // Closed before any prompt here, the Session may have no transcript (a new one, or one just opened in clay): with
      // the connection and its Pane gone, nothing is left to show on its page.
      const now = get().conns[conn] ?? c;
      const open = useUi.getState().sessionId;
      if (open != null && [conn, now.sessionId].includes(open) && now.pending == null && now.lastTurn == null) {
        const gone = isNewUnprompted(now) || (await api.getSession(open).then(() => false, isNotFound));
        if (gone && useUi.getState().sessionId === open) useUi.getState().selectSession(null);
      }
      await get().disconnect(conn);
    },

    async reconnect(conn) {
      const c = get().conns[conn];
      if (!c?.holderId) return;
      void api.ccrpcGuiLog(conn, `reconnect ${c.holderId}`);
      await closeAndWait(conn);
      try {
        addConn(await api.ccrpcConnect(c.holderId), c.holderId);
      } catch (e) {
        setErr(errorKey(c), `重连失败：${errorText(e)}`);
      }
    },

    async refreshTasks(conn) {
      const r = await get().command(conn, { type: "get_tasks" });
      if (r.success && r.data && Array.isArray(r.data.tasks)) {
        const tasks = r.data.tasks as Task[];
        update(conn, (c) => ({ ...c, tasks }));
      }
    },

    dropRequest(conn, requestId) {
      update(conn, (c) => ({ ...c, requests: c.requests.filter((q) => q.request_id !== requestId) }));
    },

    async rewind(conn, uuid) {
      const c = get().conns[conn];
      if (!c) return;
      void api.ccrpcGuiLog(conn, `rewind ${uuid}`);
      setErr(errorKey(c), null);
      update(conn, (x) => ({ ...x, picking: false, rewinding: { uuid, waited: false } }));
      const r = await get().command(conn, { type: "rewind", uuid });
      if (r.success && r.data?.type === "rewind")
        return update(conn, (x) => rewindShown(x, r.data as unknown as RewindRequest));
      update(conn, (x) => ({ ...x, rewinding: null }));
      if (!r.success) setErr(errorKey(c), `回退失败：${r.error?.message ?? r.error?.code ?? ""}`);
    },

    async answerRewind(conn, req, option) {
      const key = `${conn}|${req.request_id}`;
      if (answering.has(key)) return;
      answering.add(key);
      const msg = option ? { option } : { cancel: true };
      // A Fork's own rewind: its `fork` event (which ends the Fork) may come before this answer.
      const forking = !!get().conns[conn]?.forkOf;
      const r = await get()
        .command(conn, { type: "answer_rewind", request_id: req.request_id, ...msg })
        .finally(() => answering.delete(key));
      const c = get().conns[conn];
      if (!c) return;
      if (!r.success) {
        // The confirm page is gone (answered or left in the terminal): so is its card. A Fork's own rewind
        // refused before any key still waits in ccrpc (answered again); once the Fork failed nothing is said here.
        if (forking)
          return c.forkOf ? setErr(errorKey(c), `回退失败：${r.error?.message ?? r.error?.code ?? ""}`) : undefined;
        if (r.error?.code === "unknown_request" || r.error?.code === "not_waiting")
          get().dropRequest(conn, req.request_id);
        return setErr(errorKey(c), `${option ? "回退" : "取消回退"}失败：${r.error?.message ?? r.error?.code ?? ""}`);
      }
      if (!option || forking) return get().dropRequest(conn, req.request_id);
      // Claude Code put the prompt back into its input box; here it goes into an empty one, a draft stays.
      const dk = draftKey(c);
      const draft = get().drafts[dk] ?? EMPTY_DRAFT;
      const text = typeof r.data?.text === "string" ? r.data.text : null;
      const empty = !draft.text.trim() && draft.images.length === 0;
      if (text && empty) get().setDraft(dk, (d) => ({ ...d, text }));
      update(conn, (x) => rewindAnswered(x, req, option, !!text && !empty));
    },

    setPicking(conn, on) {
      update(conn, (c) => (c.picking === on ? c : { ...c, picking: on }));
    },

    clearRewound(conn) {
      update(conn, (c) => (c.rewound ? { ...c, rewound: null } : c));
    },

    setPending(conn, p) {
      update(conn, (c) => (c.pending === p ? c : { ...c, pending: p }));
    },

    withdrawnFound(conn, id) {
      const sid = get().conns[conn]?.sessionId;
      update(conn, (c) => (c.withdrawnText ? { ...c, withdrawnText: null } : c));
      if (!sid) return;
      const all = addWithdrawn(get().withdrawn, sid, id);
      if (all === get().withdrawn) return;
      set({ withdrawn: all });
      saveWithdrawn(all);
    },

    rewoundSent(conn) {
      update(conn, (c) => (c.rewound?.keptDraft ? { ...c, rewound: { ...c.rewound, keptDraft: false } } : c));
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
      saveDrafts(get().drafts);
    },
  };
});

/** Focuses the input box of `conn` once its panel is on screen (the Session was just selected), for up to 3 s. */
function focusInput(conn: string, tries = 60) {
  const el = document.querySelector<HTMLElement>(`[data-conn="${conn}"] [data-testid=connect-input]`);
  if (el) return el.focus();
  if (tries > 0) window.setTimeout(() => focusInput(conn, tries - 1), 50);
}

const lastLine = (s: string) => s.trim().split("\n").pop()?.trim() ?? "";

/** Sessions waiting for an answer: CoDeck's Live Sessions that wait, and connected Sessions with a card. */
export function waitingSessions(liveWaiting: string[], conns: Record<string, ConnState>): string[] {
  const out = new Set(liveWaiting);
  for (const c of Object.values(conns)) if (!c.procExit && c.requests.length > 0) out.add(c.sessionId ?? c.conn);
  return [...out];
}
