import type { Transcript } from "@/ipc/bindings";
import { type Grouping, locate } from "./grouping";
import type { PendingScroll } from "./nodes/scroll";

/** A scroll request whose node is missing waits this long / this many revisions for it to arrive. */
export const SCROLL_RETRY_MS = 5000;
export const SCROLL_RETRY_REVISIONS = 2;

/** The pending request whose node was not found yet: revisions seen since, for the retry bound. */
export interface ScrollMiss {
  seq: number;
  revision: Transcript["revision"];
  changes: number;
}

export type ScrollResolution =
  /** In the data: open these expansion keys, then scroll to it. */
  | { kind: "found"; expand: string[] }
  /** Not in the data yet: keep the request (`first`: the first miss, which starts the time bound). */
  | { kind: "wait"; miss: ScrollMiss; first: boolean }
  /** Still missing after `SCROLL_RETRY_REVISIONS` data updates: drop the request. */
  | { kind: "drop"; miss: ScrollMiss };

/**
 * Where a scroll request stands against the data: found, or missing — the node may arrive with the next data
 * update (live append, refetch), so the request is kept until the data has changed `SCROLL_RETRY_REVISIONS`
 * times. `full` must be built with `inheritedExpanded: true` (see `locate`); `miss` is the previous result's.
 */
export function resolveScrollTarget(
  full: Grouping,
  req: PendingScroll,
  revision: Transcript["revision"],
  miss: ScrollMiss | null,
): ScrollResolution {
  const loc = locate(full, req.nodeId, req.toolUseId, { thinking: req.thinking });
  if (loc) return { kind: "found", expand: loc.expand };
  if (miss?.seq !== req.seq) return { kind: "wait", miss: { seq: req.seq, revision, changes: 0 }, first: true };
  if (miss.revision === revision) return { kind: "wait", miss, first: false };
  const next = { seq: miss.seq, revision, changes: miss.changes + 1 };
  return next.changes >= SCROLL_RETRY_REVISIONS
    ? { kind: "drop", miss: next }
    : { kind: "wait", miss: next, first: false };
}
