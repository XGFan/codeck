// Rewind and Fork on the iPhone app's Transcript page (transcript.html in a WKWebView). The page holds no Connect: the
// app tells it what the buttons under a prompt show (`window.codeckPage.setConnect`), and a button hands its prompt
// to the app, which asks and acts natively.
import { create } from "zustand";
import type { CodeCall } from "./conn";

/** The app's Connect of the Session on the page, as much as the buttons need. */
export interface NativeConnect {
  /** A Connect that takes commands drives the Session: its Main Line prompts offer 「回退」. */
  connected: boolean;
  /** Idle, so a rewind can start. */
  idle: boolean;
  /** The prompt whose confirm page a `rewind` is opening. */
  rewinding: string | null;
  /** The prompt the conversation was rewound to. */
  rewound: { uuid: string; withCode: boolean } | null;
  /** A Fork can start (the Session's directory is known and there, ccrpc works). */
  canFork: boolean;
  /** A Fork of this Session is on its way. */
  forking: boolean;
  /** Code Mode: the calls the running code made that the transcript does not have yet (the Host folds them). */
  codeCalls: CodeCall[];
}

declare global {
  interface Window {
    codeckPage?: { setConnect(c: Partial<NativeConnect>): void };
  }
}

// No `window` where the unit tests load this module.
const app = () => (typeof window === "undefined" ? undefined : window.webkit?.messageHandlers?.codeck);

const NONE: NativeConnect = {
  connected: false,
  idle: false,
  rewinding: null,
  rewound: null,
  canFork: false,
  forking: false,
  codeCalls: [],
};

/** `c` is null outside the app. */
export const useNativeConnect = create<{ c: NativeConnect | null }>(() => ({ c: app() ? NONE : null }));

/** Before the page tells the app it is ready. */
export function installNativeConnect() {
  if (!app()) return;
  window.codeckPage = { setConnect: (c) => useNativeConnect.setState({ c: { ...NONE, ...c } }) };
}

/** 「回退」 or 「Fork」 was tapped under the prompt `nodeId`. */
export const askApp = (type: "rewind" | "fork", nodeId: string) => app()?.postMessage({ type, nodeId });

/** A button under a prompt on the phone: always shown (nothing hovers), tall enough for a finger. */
export const NATIVE_ACTION =
  "flex h-[30px] items-center gap-1 rounded-md px-2 text-[13px] text-secondary active:bg-selection disabled:opacity-40";
