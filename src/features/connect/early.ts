import type { InMsg } from "./protocol";

/** One thing ccrpc said on a connection: a line, or its exit. */
export type Early = { kind: "line"; msg: InMsg } | { kind: "exit"; code: number | null; stderr: string };

/**
 * Holds what arrives for a connection id that is not known yet: Tauri does not promise that the command
 * starting a ccrpc returns before its first events, and a ccrpc refused at once may even report its exit
 * first. Bounded: per id the oldest lines give way (an exit, being last, stays), and past `maxIds` ids
 * the oldest id is dropped.
 */
export function createEarlyBuffer(perId = 500, maxIds = 16) {
  const held = new Map<string, Early[]>();
  return {
    push(conn: string, e: Early) {
      let q = held.get(conn);
      if (!q) {
        if (held.size >= maxIds) held.delete(held.keys().next().value!);
        q = [];
        held.set(conn, q);
      }
      if (q.length >= perId) q.shift();
      q.push(e);
    },
    /** What was held for `conn`, in arrival order; forgets it. */
    take(conn: string): Early[] {
      const q = held.get(conn) ?? [];
      held.delete(conn);
      return q;
    },
  };
}
