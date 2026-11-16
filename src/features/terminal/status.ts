// What a terminal window shows about its Session, decided in the main window (which keeps the Connect state).

import type { ConnState } from "@/features/connect/conn";
import type { LsEntry } from "@/features/connect/protocol";
import { isLive } from "@/features/connect/store";

/**
 * - `close`: the Pane is gone — no Session runs in it any more (not in `ccrpc ls`) and no live connection is on it:
 *   the window closes. A Disconnect alone does not close it (the window does not need the connection).
 * - `screen`: a screen ccrpc cannot read is waiting (the card the window may have been opened from).
 * - `input`: Claude Code is back at its input box (connected, idle or working, nothing unread on screen).
 * - `""`: anything else (an approval or a question waiting, starting up, not connected).
 */
export type TermStatus = "close" | "screen" | "input" | "";

export function termStatus(conns: Record<string, ConnState>, ls: LsEntry[], holderId: string): TermStatus {
  const c = Object.values(conns).find((c) => c.holderId === holderId && isLive(c));
  if (!c) return ls.some((e) => e.holder_id === holderId) ? "" : "close";
  if (c.requests.some((r) => r.type === "dialog" || r.type === "unrecognized")) return "screen";
  if (c.state === "idle" || c.state === "busy") return "input";
  return "";
}
