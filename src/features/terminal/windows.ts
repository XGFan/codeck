// The main window's side of the terminal windows: opening one, telling it what its Session is doing, and
// closing it when its Pane is gone (the Session in it ended, the Pane closed).

import { useEffect } from "react";
import type { ConnState } from "@/features/connect/conn";
import type { LsEntry } from "@/ipc/protocol/ccrpc";
import { useConnect } from "@/features/connect/store";
import { termAvailable, termBridge } from "./bridge";
import { termStatus, type TermStatus } from "./status";

/** What the main window knows of one terminal window: the status last sent, and whether its Pane was seen live. */
export interface Tracked {
  sent: TermStatus | null;
  seen: boolean;
}

/**
 * The next step for one window. It closes only after its Pane was seen running: a window found after the main window
 * reloaded waits for `ccrpc ls` and the connections to come back. A status is sent only when it changed (the Connect
 * state changes with every ccrpc line).
 */
export function step(t: Tracked, status: TermStatus): { close: true } | { close: false; send: boolean; next: Tracked } {
  if (status === "close") return t.seen ? { close: true } : { close: false, send: false, next: t };
  return { close: false, send: status !== t.sent, next: { sent: status, seen: true } };
}

const tracked = new Map<string, Tracked>();

function sync({ conns, ls }: { conns: Record<string, ConnState>; ls: LsEntry[] }) {
  for (const [holderId, t] of tracked) {
    const status = termStatus(conns, ls, holderId);
    const s = step(t, status);
    if (s.close) {
      tracked.delete(holderId);
      void termBridge.closeWindow(holderId).catch(() => {});
      continue;
    }
    tracked.set(holderId, s.next);
    if (s.send) void termBridge.setStatus(holderId, status).catch(() => {});
  }
}

/** Opens the Pane's terminal window, or brings the one already open to the front. */
export async function openTerminalWindow(holderId: string, title: string) {
  await termBridge.openWindow(holderId, title);
  tracked.set(holderId, { sent: null, seen: tracked.get(holderId)?.seen ?? false });
  sync(useConnect.getState());
}

/** Keeps the open terminal windows in step with the Connect state and `ccrpc ls` (mounted once, in the main window). */
export function useTerminalWindows() {
  useEffect(() => {
    if (!termAvailable) return;
    void termBridge
      .windows()
      .then((ids) => {
        for (const id of ids) if (!tracked.has(id)) tracked.set(id, { sent: null, seen: false });
        sync(useConnect.getState());
      })
      .catch(() => {});
    return useConnect.subscribe(sync);
  }, []);
}
