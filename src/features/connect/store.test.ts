// The Connect store against a fake backend: early events, Resume's failure, replacing a connection.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InMsg, TurnEndEvent } from "@/ipc/protocol/ccrpc";

type Fn = (...a: never[]) => unknown;
const h = vi.hoisted(() => ({ api: {} as Record<string, Fn> }));
vi.mock("@/ipc", () => ({ api: h.api }));

const SID = "11111111-2222-3333-4444-555555555555";

let line: (conn: string, msg: InMsg) => void;
let exit: (conn: string, code: number | null, stderr: string) => void;
let remoteConn: (conn: string, holderId: string | null) => void;
let closing: (conn: string) => void;
/** What `connectSnapshot` answers. */
let snapshot: () => Promise<string>;
/** Whether a ccrpc exits when disconnected (else it hangs on). */
let exitsOnDisconnect: boolean;
/** Whether commands are answered. */
let answers: boolean;
let sent: { conn: string; msg: Record<string, unknown> }[];
let store: typeof import("./store");

const state = () => store.useConnect.getState();
const ready = (holderId: string, sessionId: string | null, protocol = 6): InMsg => ({
  type: "ready",
  protocol,
  claude_version: "2",
  warnings: [],
  state: "idle",
  waiting: null,
  requests: [],
  holder_id: holderId,
  session_id: sessionId,
  cwd: "/w",
  size: [80, 24],
  mode: null,
  model: null,
  effort: null,
  context_tokens: null,
  title: null,
  queue: [],
  tasks: [],
  code_calls: [],
  feedback_draft: null,
});
const flush = () => vi.advanceTimersByTimeAsync(0);

beforeEach(async () => {
  vi.useFakeTimers();
  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => void storage.set(k, v),
    removeItem: (k: string) => void storage.delete(k),
  });
  vi.stubGlobal("window", {
    setTimeout: (f: () => void, ms?: number) => setTimeout(f, ms),
    clearTimeout: (t?: number) => clearTimeout(t),
    addEventListener: () => {},
  });
  vi.stubGlobal("document", {
    hidden: false,
    visibilityState: "visible",
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  exitsOnDisconnect = true;
  answers = true;
  sent = [];
  snapshot = async () => "[]";
  for (const k of Object.keys(h.api)) delete h.api[k];
  Object.assign(h.api, {
    onCcrpcLine: (cb: typeof line) => ((line = cb), Promise.resolve(() => {})),
    onCcrpcExit: (cb: typeof exit) => ((exit = cb), Promise.resolve(() => {})),
    onLiveChanged: () => Promise.resolve(() => {}),
    onCcrpcLs: () => Promise.resolve(() => {}),
    onCcrpcLsEnded: () => Promise.resolve(() => {}),
    onCcrpcRemoteConn: (cb: typeof remoteConn) => ((remoteConn = cb), Promise.resolve(() => {})),
    onCcrpcClosing: (cb: typeof closing) => ((closing = cb), Promise.resolve(() => {})),
    connectSnapshot: () => snapshot(),
    ccrpcGuiLog: async () => {},
    ccrpcSend: async (conn: string, msg: Record<string, unknown>) => {
      sent.push({ conn, msg });
      if (!answers) return;
      queueMicrotask(() =>
        line(conn, { type: "response", id: msg.id, command: msg.type, success: true, data: {} } as unknown as InMsg),
      );
    },
    ccrpcDisconnect: async (conn: string) => {
      if (exitsOnDisconnect) queueMicrotask(() => exit(conn, 0, ""));
    },
  });
  vi.resetModules();
  store = await import("./store");
  state().start();
  await flush();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("events that arrive before the connection id", () => {
  it("lines are replayed once ccrpcConnect returns", async () => {
    let id!: (conn: string) => void;
    h.api.ccrpcConnect = () => new Promise<string>((r) => (id = r));
    const done = state().connect("H1", SID);
    await flush();
    line("c1", ready("H1", SID));
    expect(state().conns.c1).toBeUndefined();
    id("c1");
    await done;
    expect(state().conns.c1).toMatchObject({ ready: true, holderId: "H1", sessionId: SID, procExit: null });
  });

  it("an exit that came first marks the connection exited", async () => {
    let id!: (conn: string) => void;
    h.api.ccrpcConnect = () => new Promise<string>((r) => (id = r));
    const done = state().connect("H1", SID);
    await flush();
    exit("c1", 1, "refused");
    id("c1");
    await done;
    expect(state().conns.c1?.procExit).toEqual({ code: 1, stderr: "refused" });
  });
});

describe("connections shared with clients on the LAN", () => {
  /** A connection as the Host's `connectSnapshot` lists it. */
  const hostConn = (o: Record<string, unknown> = {}) => ({
    conn: "c1",
    holderId: "H1",
    sessionId: SID,
    cwd: "/w",
    claudeVersion: "2",
    ready: true,
    state: "waiting",
    waiting: null,
    mode: "manual",
    model: null,
    effort: null,
    contextTokens: null,
    requests: [{ type: "permission_request", request_id: "r1" }],
    progress: null,
    queue: [],
    codeCalls: [],
    tasks: [],
    lastTurn: null,
    procExit: null,
    startFailed: null,
    fatal: null,
    sessionExited: false,
    notices: [],
    feedbackDraft: null,
    fork: { session_id: "other" },
    disconnected: false,
    ...o,
  });
  /** A window that starts over with the Host already holding `list`. */
  async function restart(list: unknown[]) {
    snapshot = async () => JSON.stringify(list);
    vi.resetModules();
    store = await import("./store");
    state().start();
    await flush();
  }

  it("a connection started elsewhere replays what came before its id", async () => {
    line("c1", ready("H1", SID));
    remoteConn("c1", "H1");
    expect(state().conns.c1).toMatchObject({ ready: true, holderId: "H1", sessionId: SID });
  });

  it("getting a connection already here keeps its state", async () => {
    remoteConn("c1", "H1");
    line("c1", ready("H1", SID));
    line("c1", { type: "permission_request", request_id: "r1" } as unknown as InMsg);
    h.api.ccrpcConnect = async () => "c1";
    await state().connect("H1", SID);
    remoteConn("c1", "H1");
    expect(state().conns.c1).toMatchObject({ ready: true, sessionId: SID, requests: [{ request_id: "r1" }] });
  });

  it("after a Reload the Host's connections are back: ready, cards kept, nothing of theirs that is not this window's", async () => {
    await restart([hostConn(), hostConn({ conn: "c2", disconnected: true })]);
    expect(state().conns.c1).toMatchObject({ ready: true, state: "waiting", requests: [{ request_id: "r1" }] });
    expect(state().conns.c1).not.toHaveProperty("fork");
    expect(state().conns.c1).not.toHaveProperty("disconnected");
    expect(state().conns.c2).toBeUndefined();
    // Live: the next event folds onto it.
    line("c1", { type: "state", state: "idle" } as unknown as InMsg);
    expect(state().conns.c1).toMatchObject({ ready: true, state: "idle" });
  });

  it("what came while the snapshot was read is applied after it", async () => {
    let answer!: (json: string) => void;
    snapshot = () => new Promise((r) => (answer = r));
    vi.resetModules();
    store = await import("./store");
    state().start();
    await flush();
    // Claimed mid-read, and already `ready` long ago: only the snapshot knows that.
    remoteConn("c1", "H1");
    line("c1", { type: "state", state: "idle" } as unknown as InMsg);
    answer(JSON.stringify([hostConn()]));
    await flush();
    expect(state().conns.c1).toMatchObject({ ready: true, sessionId: SID, state: "idle" });
  });

  it("two starts (StrictMode) read once and keep what happened since", async () => {
    let reads = 0;
    snapshot = async () => (reads++, JSON.stringify([hostConn()]));
    vi.resetModules();
    store = await import("./store");
    state().start()();
    state().start();
    await flush();
    expect(reads).toBe(1);
    expect(state().conns.c1?.ready).toBe(true);
  });

  it("a Connect waits for the snapshot, and goes on if it fails", async () => {
    let fail!: (e: unknown) => void;
    snapshot = () => new Promise((_, r) => (fail = r));
    vi.resetModules();
    store = await import("./store");
    state().start();
    await flush();
    const connect = vi.fn(async () => "c1");
    h.api.ccrpcConnect = connect;
    const done = state().connect("H1", SID);
    await flush();
    expect(connect).not.toHaveBeenCalled();
    fail({ code: "internal", message: "x" });
    await done;
    expect(connect).toHaveBeenCalledOnce();
    expect(state().conns.c1).toBeDefined();
  });

  it("closed from elsewhere: its exit leaves no failure", async () => {
    remoteConn("c1", "H1");
    line("c1", ready("H1", SID));
    closing("c1");
    exit("c1", 0, "");
    expect(state().conns.c1).toBeUndefined();
  });

  it("closed from elsewhere after its exit: the failure goes; an unknown connection is ignored", async () => {
    remoteConn("c1", "H1");
    exit("c1", 0, "");
    expect(state().conns.c1?.procExit).toEqual({ code: 0, stderr: "" });
    closing("c1");
    expect(state().conns.c1).toBeUndefined();
    closing("nope");
    expect(state().conns.nope).toBeUndefined();
  });
});

describe("ready", () => {
  it("another protocol is said on the Session", async () => {
    h.api.ccrpcConnect = async () => "c1";
    await state().connect("H1", SID);
    line("c1", ready("H1", SID, 3));
    expect(state().errors[SID]).toBe("ccrpc 版本不匹配：协议 3，需要 6");
    line("c1", ready("H1", SID));
    expect(state().errors[SID]).toBe("ccrpc 版本不匹配：协议 3，需要 6");
  });
});

describe("commands", () => {
  it("still waiting when ccrpc exits: answered with a failure", async () => {
    h.api.ccrpcConnect = async () => "c1";
    await state().connect("H1", SID);
    line("c1", ready("H1", SID));
    answers = false;
    const r = state().command("c1", { type: "get_state" });
    exit("c1", 0, "");
    await expect(r).resolves.toMatchObject({ success: false, error: { code: "exited", message: "ccrpc 已退出" } });
  });
});

describe("rewind", () => {
  it("answers the confirm page once while an answer is under way (Esc, 「中止」 and the card's buttons)", async () => {
    h.api.ccrpcConnect = async () => "c1";
    await state().connect("H1", SID);
    line("c1", ready("H1", SID));
    const r = {
      type: "rewind",
      request_id: "rewind:1",
      uuid: "u1",
      text: "t",
      code: null,
      code_text: null,
      warning: null,
      options: [{ option: "conversation", label: "Restore conversation" }],
    } as const;
    await Promise.all([
      state().answerRewind("c1", { ...r, options: [...r.options] }, null),
      state().answerRewind("c1", { ...r, options: [...r.options] }, null),
      state().answerRewind("c1", { ...r, options: [...r.options] }, "conversation"),
    ]);
    expect(sent.filter((x) => x.msg.type === "answer_rewind").map((x) => x.msg.cancel)).toEqual([true]);
    expect(state().errors[SID]).toBeUndefined();
    // Answered: another answer goes out again.
    await state().answerRewind("c1", { ...r, options: [...r.options] }, null);
    expect(sent.filter((x) => x.msg.type === "answer_rewind")).toHaveLength(2);
  });
});

describe("Resume", () => {
  beforeEach(() => {
    h.api.ccrpcResume = async () => "r1";
  });

  it("named in time: through, and no failure later", async () => {
    await state().resume(SID, "/w");
    expect(state().resuming[SID]).toBe(true);
    expect(state().conns.r1?.resumeOf).toBe(SID);
    line("r1", ready("H9", SID));
    expect(state().resuming[SID]).toBeUndefined();
    expect(state().errors[SID]).toBeUndefined();
    expect(state().conns.r1?.sessionId).toBe(SID);
  });

  it("claude did not start (start_failed): fails with its reason, drops the connection; ccrpc's exit after it is dropped", async () => {
    await state().resume(SID, "/w");
    line("r1", ready("H9", null));
    line("r1", { type: "start_failed", code: "exited", message: "No conversation found with session ID: x" });
    expect(state().errors[SID]).toBe("Resume 失败：No conversation found with session ID: x");
    expect(state().conns.r1).toBeUndefined();
    expect(state().resuming[SID]).toBeUndefined();
    exit("r1", 1, "");
    expect(state().conns.r1).toBeUndefined();
    expect(state().errors[SID]).toBe("Resume 失败：No conversation found with session ID: x");
  });

  it("ccrpc cannot go on before naming the Session (fatal): that is the Resume's failure", async () => {
    await state().resume(SID, "/w");
    line("r1", { type: "fatal", message: "插件没有报到" });
    expect(state().errors[SID]).toBe("Resume 失败：插件没有报到");
    expect(state().conns.r1).toBeUndefined();
    expect(state().resuming[SID]).toBeUndefined();
  });

  it("one at a time per Session", async () => {
    const resume = vi.fn(async () => "r1");
    h.api.ccrpcResume = resume;
    await Promise.all([state().resume(SID, "/w"), state().resume(SID, "/w")]);
    expect(resume).toHaveBeenCalledTimes(1);
  });
});

describe("a new Session that did not start", () => {
  it("start_failed is said on its page (the connection's id); ccrpc's exit after it is no failure card's business", async () => {
    h.api.ccrpcNew = async () => "n1";
    await state().newSession("/w");
    line("n1", ready("H9", null));
    line("n1", { type: "start_failed", code: "no_claude", message: "claude 没有启动" });
    expect(state().errors.n1).toBe("新建 Session 失败：claude 没有启动");
    exit("n1", 1, "");
    expect(state().conns.n1).toMatchObject({
      startFailed: { code: "no_claude", message: "claude 没有启动" },
      procExit: { code: 1, stderr: "" },
    });
    expect(store.isLive(state().conns.n1!)).toBe(false);
  });
});

describe("a prompt taken back (Esc before any reply)", () => {
  const turnEnd = (o: Partial<TurnEndEvent>): InMsg => ({
    type: "turn_end",
    prompt_id: "p",
    trigger: "prompt",
    last_text: null,
    error: null,
    aborted: true,
    restored_text: "再看看",
    restored_uuid: "u2",
    background_tasks: [],
    duration_ms: null,
    usage: { input_tokens: 0, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
    api_cost: null,
    ...o,
  });

  it("remembers the uuid ccrpc names for the Session, once; none named or not aborted: nothing hidden", async () => {
    h.api.ccrpcConnect = async () => "c1";
    await state().connect("H1", SID);
    line("c1", ready("H1", SID));
    line("c1", turnEnd({ restored_uuid: null }));
    line("c1", turnEnd({ aborted: false }));
    expect(state().withdrawn).toEqual({});
    line("c1", turnEnd({}));
    line("c1", turnEnd({}));
    expect(state().withdrawn).toEqual({ [SID]: ["u2"] });
  });
});

describe("the Session ended", () => {
  it("ccrpc exits by itself after session_exit: the Session stays ended, nothing failed", async () => {
    h.api.ccrpcConnect = async () => "c1";
    await state().connect("H1", SID);
    line("c1", ready("H1", SID));
    line("c1", { type: "session_exit" });
    exit("c1", 0, "");
    expect(state().conns.c1).toMatchObject({ sessionExited: true, procExit: { code: 0, stderr: "" } });
    expect(state().errors[SID]).toBeUndefined();
    // Connecting again replaces it.
    h.api.ccrpcConnect = async () => "c2";
    await state().connect("H1", SID);
    expect(Object.keys(state().conns)).toEqual(["c2"]);
  });
});

describe("replacing a connection (reconnect)", () => {
  const connected = async () => {
    const connect = vi.fn(async () => (connect.mock.calls.length === 1 ? "c1" : "c2"));
    h.api.ccrpcConnect = connect;
    await state().connect("H1", SID);
    line("c1", ready("H1", SID));
    return connect;
  };

  it("waits for the old ccrpc to exit, then attaches again", async () => {
    const connect = await connected();
    await state().reconnect("c1");
    expect(connect).toHaveBeenCalledTimes(2);
    expect(Object.keys(state().conns)).toEqual(["c2"]);
  });

  it("an old ccrpc that does not exit is given up after 5 s", async () => {
    const connect = await connected();
    exitsOnDisconnect = false;
    const done = state().reconnect("c1");
    await vi.advanceTimersByTimeAsync(4_999);
    expect(connect).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    await done;
    expect(connect).toHaveBeenCalledTimes(2);
    expect(Object.keys(state().conns)).toEqual(["c2"]);
  });

  it("a backend that no longer knows the old one: dropped at once", async () => {
    const connect = await connected();
    h.api.ccrpcDisconnect = async () => Promise.reject({ code: "invalidQuery", message: "ccrpc 已退出" });
    await state().reconnect("c1");
    expect(connect).toHaveBeenCalledTimes(2);
    expect(Object.keys(state().conns)).toEqual(["c2"]);
  });

  it("the old connection's late events are dropped, not held for a new id", async () => {
    await connected();
    await state().reconnect("c1");
    exit("c1", 0, "");
    line("c1", ready("H1", SID));
    expect(state().conns.c1).toBeUndefined();
  });
});

describe("Fork", () => {
  const NEW = "99999999-2222-3333-4444-555555555555";
  beforeEach(() => {
    h.api.listSessions = async () => [];
    h.api.ccrpcFork = async () => "f1";
  });
  const forked = () =>
    line("f1", {
      type: "fork",
      session_id: NEW,
      origin: SID,
      text: "again",
      title: "t (Fork)",
      title_error: null,
    } as unknown as InMsg);

  it("its rewind's ⚠ card answered: the `fork` event and the card's end come before the answer", async () => {
    await state().fork({ sessionId: SID, cwd: "/w", title: "t", at: "u1", restore: "both" });
    line("f1", ready("H9", null));
    line("f1", { type: "started", session_id: NEW, cwd: "/w" } as unknown as InMsg);
    expect(state().forking[SID]).toEqual({ at: "u1", sessionId: NEW });
    const req = {
      type: "rewind",
      request_id: "rewind:1",
      uuid: "u1",
      text: "again",
      code: null,
      warning: "⚠",
      options: [{ option: "both", label: "Restore code and conversation" }],
    };
    line("f1", req as unknown as InMsg);
    expect(state().conns.f1?.requests.length).toBe(1);
    // As ccrpc does: the Fork is finished (and reported) before the answer's response.
    h.api.ccrpcSend = async (conn: string, msg: Record<string, unknown>) => {
      if (msg.type !== "answer_rewind") return;
      queueMicrotask(() => {
        forked();
        line(conn, { type: "request_end", request_id: "rewind:1" });
        line(conn, {
          type: "response",
          id: msg.id,
          command: msg.type,
          success: true,
          data: { state: "idle", text: "again" },
        } as unknown as InMsg);
      });
    };
    await state().answerRewind("f1", req as never, "both");
    expect(state().forking[SID]).toBeUndefined();
    expect(state().drafts[NEW]?.text).toBe("again");
    const c = state().conns.f1!;
    expect(c.requests).toEqual([]);
    expect(c.rewound).toBeNull();
    expect(state().errors[SID]).toBeUndefined();
  });

  it("its rewind's answer refused before any key: the card stays to be answered again", async () => {
    await state().fork({ sessionId: SID, cwd: "/w", title: "t", at: "u1", restore: "both" });
    line("f1", ready("H9", null));
    line("f1", { type: "started", session_id: NEW, cwd: "/w" } as unknown as InMsg);
    const req = {
      type: "rewind",
      request_id: "rewind:1",
      uuid: "u1",
      text: "again",
      code: null,
      warning: "⚠",
      options: [{ option: "both", label: "Restore code and conversation" }],
    };
    line("f1", req as unknown as InMsg);
    h.api.ccrpcSend = async (conn: string, msg: Record<string, unknown>) => {
      queueMicrotask(() =>
        line(conn, {
          type: "response",
          id: msg.id,
          command: msg.type,
          success: false,
          error: { code: "not_waiting", message: "当前状态为 idle" },
        } as unknown as InMsg),
      );
    };
    await state().answerRewind("f1", req as never, "both");
    expect(state().conns.f1?.requests.length).toBe(1);
    expect(state().forking[SID]).toBeTruthy();
  });

  it("ccrpc says why it cannot go on (fatal) before exiting: that is the Fork's reason, not stderr", async () => {
    await state().fork({ sessionId: SID, cwd: "/w", title: "t", at: null, restore: null });
    line("f1", ready("H9", null));
    line("f1", { type: "fatal", message: "插件没有报到" });
    exit("f1", 1, "ccrpc: 插件没有报到\n");
    await flush();
    expect(state().errors[SID]).toBe("Fork 失败：插件没有报到");
  });

  it("ccrpc exits before its `fork` event: failed on the Origin, the connection dropped", async () => {
    await state().fork({ sessionId: SID, cwd: "/w", title: "t", at: null, restore: null });
    line("f1", ready("H9", null));
    exit("f1", 1, "ccrpc: 找不到 Session x 的 transcript\n");
    await flush();
    expect(state().errors[SID]).toBe("Fork 失败：找不到 Session x 的 transcript");
    expect(state().forking[SID]).toBeUndefined();
    expect(state().conns.f1).toBeUndefined();
  });
});
