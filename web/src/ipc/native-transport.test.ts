// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createNativeRemoteApi } from "./remote";

interface MockWindow {
  location: { origin: string };
  codeckPage?: {
    event?: (text: string) => void;
    setConnect?: (c: unknown) => void;
  };
  webkit?: {
    messageHandlers?: {
      codeck?: { postMessage(msg: unknown): void };
      codeckRpc?: { postMessage(msg: unknown): Promise<unknown> };
    };
  };
}

let win: MockWindow;
let docAddListener: ReturnType<typeof vi.fn>;
let wsCtor: ReturnType<typeof vi.fn>;

beforeEach(() => {
  win = { location: { origin: "http://127.0.0.1:9999" } };
  docAddListener = vi.fn();
  wsCtor = vi.fn(() => {
    throw new Error("WebSocket must not be constructed in native bridge mode");
  });
  vi.stubGlobal("window", win);
  vi.stubGlobal("document", {
    addEventListener: docAddListener,
    removeEventListener: vi.fn(),
    visibilityState: "visible",
  });
  vi.stubGlobal("WebSocket", wsCtor);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

describe("native bridge transport", () => {
  it("forwards RPC calls to codeckRpc and returns result", async () => {
    const postMessage = vi.fn<(msg: unknown) => Promise<unknown>>(async () => ({ result: ["p1"] }));
    win.webkit = { messageHandlers: { codeckRpc: { postMessage } } };

    const api = createNativeRemoteApi();
    await expect(api.listProjects()).resolves.toEqual(["p1"]);
    expect(postMessage).toHaveBeenCalledWith({ method: "list_projects", args: {} });

    postMessage.mockResolvedValueOnce({ result: { id: "s1" } });
    await expect(api.getSession("s1")).resolves.toEqual({ id: "s1" });
    expect(postMessage).toHaveBeenLastCalledWith({ method: "get_session", args: { sessionId: "s1" } });
  });

  it("rejects with the bridge error object", async () => {
    const postMessage = vi.fn(async () => ({
      error: { code: "not_found", message: "没有这个 Session" },
    }));
    win.webkit = { messageHandlers: { codeckRpc: { postMessage } } };

    const api = createNativeRemoteApi();
    await expect(api.getSession("missing")).rejects.toEqual({
      code: "not_found",
      message: "没有这个 Session",
    });
  });

  it("maps postMessage rejection and synchronous throw to code http", async () => {
    const postMessage = vi.fn(async () => {
      throw new Error("bridge disconnected");
    });
    win.webkit = { messageHandlers: { codeckRpc: { postMessage } } };

    const api = createNativeRemoteApi();
    await expect(api.listProjects()).rejects.toEqual({ code: "http", message: "连不上 Host" });

    postMessage.mockImplementationOnce(() => {
      throw new Error("sync throw");
    });
    await expect(api.listProjects()).rejects.toEqual({ code: "http", message: "连不上 Host" });
  });

  it("dispatches pushed events and triggers onResync on hello without creating WebSocket or visibilitychange", async () => {
    const postMessage = vi.fn(async () => ({ result: null }));
    win.webkit = { messageHandlers: { codeckRpc: { postMessage } } };

    const api = createNativeRemoteApi();
    expect(typeof win.codeckPage?.event).toBe("function");

    const sessions = vi.fn();
    const resync = vi.fn();
    const unlistenSessions = await api.onSessionsChanged(sessions);
    const unlistenResync = api.onResync(resync);

    win.codeckPage!.event!(JSON.stringify({ event: "hello", payload: { apiVersion: 2 } }));
    expect(resync).toHaveBeenCalledTimes(1);
    expect(sessions).not.toHaveBeenCalled();

    const payload = { changed: ["s1"], removed: [] };
    win.codeckPage!.event!(JSON.stringify({ event: "sessions-changed-event", payload }));
    expect(sessions).toHaveBeenCalledWith(payload);

    // Malformed frames are ignored.
    win.codeckPage!.event!("not json");
    win.codeckPage!.event!("null");
    expect(sessions).toHaveBeenCalledTimes(1);
    expect(resync).toHaveBeenCalledTimes(1);

    unlistenSessions();
    unlistenResync();
    win.codeckPage!.event!(JSON.stringify({ event: "hello", payload: { apiVersion: 2 } }));
    win.codeckPage!.event!(JSON.stringify({ event: "sessions-changed-event", payload }));
    expect(sessions).toHaveBeenCalledTimes(1);
    expect(resync).toHaveBeenCalledTimes(1);

    expect(wsCtor).not.toHaveBeenCalled();
    expect(docAddListener).not.toHaveBeenCalled();
  });

  it("copyText still posts to the codeck message handler", async () => {
    const codeckPost = vi.fn();
    win.webkit = {
      messageHandlers: {
        codeck: { postMessage: codeckPost },
        codeckRpc: { postMessage: vi.fn(async () => ({ result: null })) },
      },
    };

    const api = createNativeRemoteApi();
    await api.copyText("hello");
    expect(codeckPost).toHaveBeenCalledWith({ type: "copyText", text: "hello" });
  });

  it("selects native transport in ipc/index when VITE_IPC=remote and codeckRpc is present", async () => {
    const postMessage = vi.fn(async () => ({ result: ["native"] }));
    win.webkit = { messageHandlers: { codeckRpc: { postMessage } } };
    vi.stubEnv("VITE_IPC", "remote");
    vi.resetModules();

    const { api } = await import("./index");
    expect(typeof win.codeckPage?.event).toBe("function");
    await expect(api.listProjects()).resolves.toEqual(["native"]);
    expect(postMessage).toHaveBeenCalledWith({ method: "list_projects", args: {} });
    expect(wsCtor).not.toHaveBeenCalled();
  });

  it("falls back to HTTP transport in ipc/index when VITE_IPC=remote and codeckRpc is absent", async () => {
    const fetchMock = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({ result: ["http"] }),
    }));
    vi.stubGlobal("fetch", fetchMock);
    vi.stubEnv("VITE_IPC", "remote");
    vi.resetModules();

    const { api } = await import("./index");
    expect(win.codeckPage?.event).toBeUndefined();
    await expect(api.listProjects()).resolves.toEqual(["http"]);
    expect(fetchMock).toHaveBeenCalledWith("http://127.0.0.1:9999/v1/rpc", expect.objectContaining({ method: "POST" }));
  });
});
