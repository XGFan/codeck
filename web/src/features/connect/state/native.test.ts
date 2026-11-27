// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createNativeRemoteApi } from "@/ipc/remote";
import { askApp, installNativeConnect, useNativeConnect } from "./native";

interface MockWindow {
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
let codeckPost: ReturnType<typeof vi.fn<(msg: unknown) => void>>;

beforeEach(() => {
  codeckPost = vi.fn<(msg: unknown) => void>();
  win = {
    webkit: {
      messageHandlers: {
        codeck: { postMessage: codeckPost },
        codeckRpc: { postMessage: vi.fn(async () => ({ result: null })) },
      },
    },
  };
  vi.stubGlobal("window", win);
  useNativeConnect.setState({ c: null });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("window.codeckPage coexistence", () => {
  it("keeps both event and setConnect when event is installed first", () => {
    const api = createNativeRemoteApi();
    const pageObj = win.codeckPage;
    expect(pageObj).toBeDefined();

    installNativeConnect();
    expect(win.codeckPage).toBe(pageObj);
    expect(typeof win.codeckPage?.event).toBe("function");
    expect(typeof win.codeckPage?.setConnect).toBe("function");

    const resync = vi.fn();
    const unsub = api.onResync(resync);
    win.codeckPage!.event!(JSON.stringify({ event: "hello", payload: { apiVersion: 2 } }));
    expect(resync).toHaveBeenCalledTimes(1);
    unsub();

    win.codeckPage!.setConnect!({ connected: true, idle: true });
    expect(useNativeConnect.getState().c?.connected).toBe(true);
    expect(useNativeConnect.getState().c?.idle).toBe(true);

    askApp("rewind", "n1");
    expect(codeckPost).toHaveBeenCalledWith({ type: "rewind", nodeId: "n1" });
  });

  it("keeps both setConnect and event when setConnect is installed first", () => {
    installNativeConnect();
    const pageObj = win.codeckPage;
    expect(pageObj).toBeDefined();

    const api = createNativeRemoteApi();
    expect(win.codeckPage).toBe(pageObj);
    expect(typeof win.codeckPage?.setConnect).toBe("function");
    expect(typeof win.codeckPage?.event).toBe("function");

    win.codeckPage!.setConnect!({ connected: true, canFork: true });
    expect(useNativeConnect.getState().c?.connected).toBe(true);
    expect(useNativeConnect.getState().c?.canFork).toBe(true);

    const resync = vi.fn();
    const unsub = api.onResync(resync);
    win.codeckPage!.event!(JSON.stringify({ event: "hello", payload: { apiVersion: 2 } }));
    expect(resync).toHaveBeenCalledTimes(1);
    unsub();
  });
});
