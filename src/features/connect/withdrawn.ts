// A prompt taken back: Esc before any reply makes Claude Code put it back in its input box and out of the
// conversation, while the transcript keeps it (and the next prompt follows it with no reply between, which is also how
// two prompts sent in quick succession and answered together look: the transcript alone cannot tell them apart). ccrpc's
// aborted `turn_end` says so (`restored_text`, the input box showing the prompt again); the prompt is found by that text
// and hidden from then on, remembered per Session.

import type { Node } from "@/ipc/bindings";

const squeeze = (t: string) => t.replace(/\s+/g, "");

/** The id of the last typed prompt when it has no reply after it and reads `text` (spacing aside), else null. */
export function withdrawnPrompt(nodes: readonly Node[], text: string): string | null {
  for (let i = nodes.length - 1; i >= 0; i--) {
    const n = nodes[i]!;
    if (n.body.kind === "assistant") return null;
    if (n.body.kind !== "userPrompt" || n.hidden) continue;
    if (n.body.origin.kind !== "human") return null;
    return squeeze(n.body.text) === squeeze(text) ? n.id : null;
  }
  return null;
}

const KEY = "codeck.withdrawn";
/** Sessions remembered, newest last (the oldest go). */
const MAX_SESSIONS = 200;

/** The prompts taken back, by Session; empty when storage is unavailable or garbled. */
export function loadWithdrawn(): Record<string, string[]> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    if (!v || typeof v !== "object" || Array.isArray(v)) return {};
    return Object.fromEntries(
      Object.entries(v).filter(([, ids]) => Array.isArray(ids) && ids.every((i) => typeof i === "string")),
    );
  } catch {
    return {};
  }
}

/** `all` with `id` added to `sessionId`'s (that Session moves to the end), within [`MAX_SESSIONS`]. */
export function addWithdrawn(all: Record<string, string[]>, sessionId: string, id: string): Record<string, string[]> {
  const { [sessionId]: ids = [], ...rest } = all;
  if (ids.includes(id)) return all;
  const kept = Object.entries(rest).slice(-(MAX_SESSIONS - 1));
  return { ...Object.fromEntries(kept), [sessionId]: [...ids, id] };
}

export function saveWithdrawn(all: Record<string, string[]>) {
  try {
    localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    // Not kept past this run.
  }
}

/** `nodes` with those in `ids` hidden (shown only with the hidden entries). */
export function hideNodes(nodes: Node[], ids: readonly string[]): Node[] {
  if (ids.length === 0) return nodes;
  const set = new Set(ids);
  return nodes.map((n) => (set.has(n.id) && !n.hidden ? { ...n, hidden: true } : n));
}
