import { createContext, useContext } from "react";
import { create } from "zustand";
import type { SubagentRun, TranscriptScope } from "@/ipc/bindings";
import { useUi } from "@/state/ui";
import { anchorKey } from "../grouping";

/** A request to reveal a node (and optionally one tool call) in the transcript list of `scope`. */
export interface ScrollRequest {
  scope: TranscriptScope;
  nodeId: string;
  toolUseId?: string | null;
  /** Also expand the node's thinking blocks (find match inside thinking). */
  thinking?: boolean;
  /** How long the highlight ring stays, in ms; `null` keeps it until the next highlight. Default 2000. */
  flashMs?: number | null;
}

export interface PendingScroll extends ScrollRequest {
  seq: number;
}

export const scopeKey = (s: TranscriptScope) => (s.kind === "main" ? "main" : `agent:${s.agentId}`);

const useScrollRequests = create<{ byScope: Record<string, PendingScroll> }>(() => ({ byScope: {} }));
let seq = 0;

/**
 * Queues a scroll for the transcript list showing `req.scope`; the list consumes it once its data contains the
 * node: expands the inherited prefix / group / tool / thinking / compact summary as needed, scrolls the
 * virtualizer to the row (centered), then sets `useUi.highlight` and clears it after `flashMs`.
 * Requests for a scope that is not on screen wait until that list mounts (e.g. the Subagent panel opening).
 */
export function scrollToNode(req: ScrollRequest): void {
  const p = { ...req, seq: ++seq };
  useScrollRequests.setState((s) => ({ byScope: { ...s.byScope, [scopeKey(req.scope)]: p } }));
}

export const usePendingScroll = (scope: TranscriptScope) => useScrollRequests((s) => s.byScope[scopeKey(scope)]);

export function clearScroll(scope: TranscriptScope, seqNo: number) {
  useScrollRequests.setState((s) => {
    const k = scopeKey(scope);
    if (s.byScope[k]?.seq !== seqNo) return s;
    const { [k]: _, ...rest } = s.byScope;
    return { byScope: rest };
  });
}

/** True while `useUi.highlight` points at this node / call. */
export function useHighlighted(nodeId: string, toolUseId?: string | null): boolean {
  return useUi((s) => {
    const h = s.highlight;
    if (!h || h.nodeId !== nodeId) return false;
    return toolUseId ? h.toolUseId === toolUseId : !h.toolUseId;
  });
}

export const anchorAttr = (nodeId: string, toolUseId?: string | null) => ({
  "data-anchor": anchorKey(nodeId, toolUseId),
});

/** Shared by every row of one transcript list (main or Subagent panel). */
export interface TranscriptCtx {
  sessionId: string;
  scope: TranscriptScope;
  /** Subagent Runs known for the session (main transcript + the panel transcript). */
  runs: Map<string, SubagentRun>;
  /** toolUseId → node id, for card ↔ notification links. */
  toolNode: Map<string, string>;
  /** Opens a Subagent Run in the right panel (replaces the stack from main, pushes from inside the panel). */
  openAgent: (agentId: string) => void;
  /** Innermost Subagent Run shown in the panel. */
  activeAgentId: string | null;
  /** Prompts offering 「回退」 (the Main Line's in a Connected Session's main transcript). */
  rewindable: ReadonlySet<string>;
  /** Another Session's socket → its name, from the messages it sent (for SendMessage to that Session). */
  peerNames: ReadonlyMap<string, string>;
}

export const TranscriptContext = createContext<TranscriptCtx | null>(null);

export function useTranscriptCtx(): TranscriptCtx {
  const c = useContext(TranscriptContext);
  if (!c) throw new Error("TranscriptContext missing");
  return c;
}

const bottomListeners = new Set<() => void>();

/** The followed transcript (the open Session's, live) jumps to its end and stays there: a message was just sent. */
export function scrollToBottom() {
  for (const f of bottomListeners) f();
}

/** `f` runs on `scrollToBottom`; returns the unsubscribe. */
export function onScrollToBottom(f: () => void): () => void {
  bottomListeners.add(f);
  return () => void bottomListeners.delete(f);
}
