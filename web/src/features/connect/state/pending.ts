// A sent prompt between Enter and its node in the indexed transcript: a placeholder bubble at the transcript's end
// (ConnectTail), never its text (the transcript draws it as the index has it). The node that replaces it plays the
// arrival once.

/** Prompts whose node, once it mounts, plays the arrival (each kept for a while only). */
const fresh = new Map<string, number>();
const FRESH_MS = 60_000;

/** `uuid`'s node will replace a placeholder: it plays the arrival when it first shows. */
export function markFresh(uuid: string) {
  const now = Date.now();
  for (const [k, t] of fresh) if (now - t > FRESH_MS) fresh.delete(k);
  fresh.set(uuid, now);
}

/** The node replaces a placeholder: true until its arrival has played (`arrived`). */
export const isFresh = (uuid: string) => fresh.has(uuid);

export const arrived = (uuid: string) => void fresh.delete(uuid);

/** How long a placeholder waits for its node before it goes (the node may never be a prompt row: a Branch view). */
export const PENDING_MAX_MS = 20_000;
