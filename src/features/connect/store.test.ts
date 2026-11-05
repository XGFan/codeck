// The Connect store against a fake backend: early events, Resume's time bound and failure, replacing a connection.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InMsg } from "./protocol";

type Fn = (...a: never[]) => unknown;
const h = vi.hoisted(() => ({ api: {} as Record<string, Fn> }));
vi.mock("@/ipc", () => ({ api: h.api }));

const SID = "11111111-2222-3333-4444-555555555555";

let line: (conn: string, msg: InMsg) => void;
let exit: (conn: string, code: number | null, stderr: string) => void;
/** What `get_screen` answers. */
let screen: { lines: string[]; cursor: [number, number] };
/** Whether a ccrpc exits when disconnected (else it hangs on). */
let exitsOnDisconnect: boolean;
/** Whether commands are answered. */
let answers: boolean;
let sent: { conn: string; msg: Record<string, unknown> }[];
let store: typeof import("./store");

const state = () => store.useConnect.getState();
const ready = (holderId: string, sessionId: string | null) =>
  ({
    type: "ready",
    protocol: 1,
    holder_id: holderId,
    session_id: sessionId,
    cwd: "/w",
    claude_version: "2",
    state: "idle",
    size: [80, 24],
    warnings: [],
  }) as unknown as InMsg;
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
  screen = { lines: [], cursor: [0, 0] };
  exitsOnDisconnect = true;
  answers = true;
  sent = [];
  for (const k of Object.keys(h.api)) delete h.api[k];
  Object.assign(h.api, {
    onCcrpcLine: (cb: typeof line) => ((line = cb), Promise.resolve(() => {})),
    onCcrpcExit: (cb: typeof exit) => ((exit = cb), Promise.resolve(() => {})),
    onLiveChanged: () => Promise.resolve(() => {}),
    ccrpcGuiLog: async () => {},
    ccrpcSend: async (conn: string, msg: Record<string, unknown>) => {
      sent.push({ conn, msg });
      if (!answers) return;
      const data = msg.type === "get_screen" ? screen : msg.type === "get_tasks" ? { tasks: [] } : {};
      queueMicrotask(() =>
        line(conn, { type: "response", id: msg.id, command: msg.type, success: true, data } as unknown as InMsg),
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
    await vi.advanceTimersByTimeAsync(31_000);
    expect(state().errors[SID]).toBeUndefined();
    expect(state().conns.r1?.sessionId).toBe(SID);
  });

  it("not named within 30 s: fails with claude's last line, closes the Pane and drops the connection", async () => {
    screen = { lines: ["% claude --resume x", "something went wrong", "% "], cursor: [2, 2] };
    await state().resume(SID, "/w");
    line("r1", ready("H9", null));
    await vi.advanceTimersByTimeAsync(29_999);
    expect(sent.filter((s) => s.msg.type !== "get_tasks")).toEqual([]);
    await vi.advanceTimersByTimeAsync(1 + 300);
    await flush();
    expect(sent.filter((s) => s.msg.type !== "get_tasks").map((s) => s.msg)).toEqual([
      expect.objectContaining({ type: "get_screen" }),
      expect.objectContaining({ type: "exit", close_pane: true, stop_tasks: true }),
    ]);
    expect(state().errors[SID]).toBe("Resume 失败：30 秒内没有启动（something went wrong）");
    expect(state().conns.r1).toBeUndefined();
    expect(state().resuming[SID]).toBeUndefined();
  });

  it("a card waiting at 30 s puts the bound off by another 30 s", async () => {
    await state().resume(SID, "/w");
    line("r1", { type: "permission_request", request_id: "p1" } as unknown as InMsg);
    await vi.advanceTimersByTimeAsync(30_400);
    expect(state().errors[SID]).toBeUndefined();
    line("r1", { type: "state", state: "idle" } as unknown as InMsg);
    await vi.advanceTimersByTimeAsync(30_400);
    expect(state().errors[SID]).toMatch(/^Resume 失败：30 秒内没有启动/);
  });

  it("claude exits before naming the Session: fails with its error line", async () => {
    screen = { lines: ["% claude --resume x", "No conversation found with session ID: x", "% "], cursor: [2, 2] };
    await state().resume(SID, "/w");
    line("r1", { type: "session_exit" } as unknown as InMsg);
    await vi.advanceTimersByTimeAsync(300);
    await flush();
    expect(state().errors[SID]).toBe("Resume 失败：No conversation found with session ID: x");
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
