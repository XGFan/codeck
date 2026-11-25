// The terminal window's own way to the backend (ADR-0015): the `codeck-term` tauri plugin, or in mock mode
// (E2E, `pnpm dev:mock`) a popup page that replays a fixed screen. Kept out of `Api` and the generated
// bindings so that the whole feature can be removed at once.

import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { message, type TermMessage } from "./messages";
import type { TermStatus } from "./status";

export interface TermBridge {
  // The main window.
  openWindow(holderId: string, title: string): Promise<void>;
  closeWindow(holderId: string): Promise<void>;
  setStatus(holderId: string, status: TermStatus): Promise<void>;
  /** The Holder IDs of the terminal windows open now. */
  windows(): Promise<string[]>;
  // A terminal window.
  /** Attaches to the Pane; resolves with the status the main window set last, if any. */
  attach(holderId: string, onMessage: (m: ArrayBuffer | Uint8Array) => void): Promise<TermStatus | null>;
  /** Bytes for the Pane, as typed. */
  input(data: Uint8Array): Promise<void>;
  onStatus(holderId: string, cb: (status: TermStatus) => void): Promise<() => void>;
  /** Sets the window's content size (logical px). */
  resizeWindow(width: number, height: number): Promise<void>;
}

const plugin = (cmd: string, args?: Record<string, unknown>) => invoke<unknown>(`plugin:codeck-term|${cmd}`, args);

function realBridge(): TermBridge {
  return {
    openWindow: async (holderId, title) => void (await plugin("open_window", { holderId, title })),
    closeWindow: async (holderId) => void (await plugin("close_window", { holderId })),
    setStatus: async (holderId, status) => void (await plugin("set_status", { holderId, status })),
    windows: async () => (await plugin("windows")) as string[],
    attach: async (holderId, onMessage) => {
      const onData = new Channel<ArrayBuffer>();
      onData.onmessage = onMessage;
      return (await plugin("attach", { holderId, onData })) as TermStatus | null;
    },
    input: async (data) => void (await plugin("input", { data: Array.from(data) })),
    onStatus: (_holderId, cb) =>
      getCurrentWebviewWindow().listen<TermStatus>("codeck-term:status", (e) => cb(e.payload)),
    resizeWindow: (width, height) => getCurrentWebviewWindow().setSize(new LogicalSize(width, height)),
  };
}

// ---- mock -------------------------------------------------------------------------------------------------

/** The screen the mock Pane shows: `/cost`'s panel, as in the mock Connect flow (`clay-new.jsonl`). */
const MOCK_SCREEN = [
  "   Settings  Status   Config   Usage   Stats",
  "",
  "   Session",
  "",
  "   Total cost:            $0.0000",
  "   Total duration (API):  0s",
  "",
  "   Esc to cancel",
];
const MOCK_SIZE = { cols: 80, rows: 24 };
const enc = new TextEncoder();
const statusChannel = () => new BroadcastChannel("codeck-term");
/** The status last set, kept like the plugin keeps it for a window that attaches later. */
const statusKey = (holderId: string) => `codeck-term-status:${holderId}`;

declare global {
  interface Window {
    /** Mock mode: what the terminal page typed into its Pane (for E2E). */
    __termInput?: string[];
  }
}

function mockBridge(): TermBridge {
  const popups = new Map<string, Window>();
  let echo: ((m: Uint8Array) => void) | null = null;
  return {
    openWindow: async (holderId, title) => {
      const open = popups.get(holderId);
      if (open && !open.closed) return open.focus();
      const url = `/terminal.html?holder=${encodeURIComponent(holderId)}&title=${encodeURIComponent(title)}`;
      const w = window.open(url, `term-${holderId}`, "popup,width=960,height=560");
      if (w) popups.set(holderId, w);
    },
    closeWindow: async (holderId) => {
      popups.get(holderId)?.close();
      popups.delete(holderId);
    },
    setStatus: async (holderId, status) => {
      localStorage.setItem(statusKey(holderId), status);
      const ch = statusChannel();
      ch.postMessage({ holderId, status });
      ch.close();
    },
    windows: async () => [...popups].filter(([, w]) => !w.closed).map(([id]) => id),
    attach: async (holderId, onMessage) => {
      echo = onMessage;
      const screen = `\x1b[2J\x1b[H${MOCK_SCREEN.join("\r\n")}`;
      const sends: TermMessage[] = [
        { kind: "size", ...MOCK_SIZE },
        { kind: "history", data: enc.encode("earlier output\r\n") },
        { kind: "snapshot", data: enc.encode(screen) },
      ];
      for (const m of sends) onMessage(message(m));
      return localStorage.getItem(statusKey(holderId)) as TermStatus | null;
    },
    input: async (data) => {
      (window.__termInput ??= []).push(new TextDecoder().decode(data));
      // The mock Pane echoes what is typed.
      echo?.(message({ kind: "output", data }));
    },
    onStatus: async (holderId, cb) => {
      const ch = statusChannel();
      ch.onmessage = (e: MessageEvent<{ holderId: string; status: TermStatus }>) => {
        if (e.data.holderId === holderId) cb(e.data.status);
      };
      return () => ch.close();
    },
    resizeWindow: async () => {},
  };
}

/** The terminal window is desktop only: not on the daemon's pages (`VITE_IPC=remote`). */
export const termAvailable = import.meta.env.VITE_IPC !== "remote";

export const termBridge: TermBridge = import.meta.env.VITE_IPC === "mock" ? mockBridge() : realBridge();
