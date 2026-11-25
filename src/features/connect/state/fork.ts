// Fork (CONTEXT.md, ADR-0009): a new Session copying the Main Line of the Origin Session up to a prompt (or all of
// it), made by `ccrpc new --fork` in a new clay Pane and Connected.
import { resumeOffer } from "./resume";

/** What a 「Fork」 offers: like Resume's, but also for a Session still running (a Fork never touches it). */
export type ForkOffer = { kind: "enabled" } | { kind: "disabled"; why: string };

export function forkOffer(o: { cwd: string | null; cwdMissing: boolean; ccrpcError: string | null }): ForkOffer {
  const r = resumeOffer({ ...o, live: false });
  return r.kind === "disabled" ? r : { kind: "enabled" };
}

const FORK_MARK = /\s*\(Fork(?: \d+)?\)$/;

/**
 * The Fork's title: the Origin Session's (on one line, without its own `(Fork)` / `(Fork N)`), then ` (Fork)`, or
 * ` (Fork 2)`, ` (Fork 3)`… when a Session already has that title.
 */
export function forkTitle(origin: string, taken: Iterable<string>): string {
  const base = origin.replace(/\s+/g, " ").trim().replace(FORK_MARK, "");
  const used = new Set(taken);
  const at = (n: number) => `${base} (Fork${n > 1 ? ` ${n}` : ""})`.trim();
  let n = 1;
  while (used.has(at(n))) n++;
  return at(n);
}
