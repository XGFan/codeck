// The hooks behind `TranscriptList`: rows, turn publishing (outline / j-k), staying at the bottom, scroll requests.
import type { Virtualizer } from "@tanstack/react-virtual";
import { type RefObject, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { SubagentRun, Transcript, TranscriptScope } from "@/ipc/bindings";
import { useUi } from "@/state/ui";
import { foldTurns } from "./fold";
import { anchorKey, buildRows, type Grouping, type GroupingInput, type Row, rowIndexOf } from "./grouping";
import { INHERITED_KEY, isFoldKey } from "./keys";
import { clearScroll, onScrollToBottom, type PendingScroll, usePendingScroll } from "./nodes/scroll";
import { resetReading, setCurrentTurn, setTurns, useReading } from "./reading";
import { resolveScrollTarget, SCROLL_RETRY_MS, type ScrollMiss } from "./scrollTarget";
import { commandLine } from "./util";

type ListVirtualizer = Virtualizer<HTMLDivElement, Element>;

const PIN_THRESHOLD = 48;
/** Keys that scroll the focused list; they end the outline jump lock like a wheel or drag does. */
const SCROLL_KEYS = new Set(["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "]);

/** The fold keys' state in `expanded`, as a string (stable while only other keys change). */
const foldState = (expanded: Record<string, boolean>) =>
  Object.keys(expanded)
    .filter(isFoldKey)
    .map((k) => `${k}=${expanded[k] ? 1 : 0}`)
    .join(",");

/** The transcript's rows: grouped (`buildRows`), then folded by turn with the fold keys' state. */
export function useRows(transcript: Transcript, agent: SubagentRun | null) {
  const showHidden = useUi((s) => s.showHidden);
  const inheritedExpanded = useUi((s) => s.expanded[INHERITED_KEY] ?? false);
  const input = useMemo<GroupingInput>(
    () => ({
      nodes: transcript.nodes,
      branchPoints: transcript.branchPoints,
      subagents: transcript.subagents,
      workflows: transcript.workflows,
      inherited: transcript.inherited,
      orphanSubagentIds: transcript.orphanSubagentIds,
      showHidden,
      inheritedExpanded,
      agent,
    }),
    [transcript, showHidden, inheritedExpanded, agent],
  );
  const built = useMemo(() => buildRows(input), [input]);
  // Only the fold keys' state: expanding a tool call or a group does not fold the rows again.
  const folds = useUi((s) => foldState(s.expanded));
  // `folds` is the trigger; the fold keys are read from the store when it changes.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const grouping = useMemo(() => foldTurns(built, useUi.getState().expanded), [built, folds]);
  return { input, grouping, inheritedExpanded };
}

/**
 * Outline / j-k support (main list only): the turns, and which one the reading line (55% down the viewport) is in;
 * a wheel, drag or scroll key ends the outline's jump lock.
 */
export function usePublishTurns(
  isMain: boolean,
  rows: Row[],
  virtualizer: ListVirtualizer,
  scrollRef: RefObject<HTMLDivElement | null>,
) {
  const publishTurn = useCallback(() => {
    const el = scrollRef.current;
    if (!isMain || !el || useReading.getState().locked) return;
    const probe = el.scrollTop + el.clientHeight * 0.55 - 20;
    let turn = -1;
    for (const v of virtualizer.getVirtualItems()) {
      if (v.start > probe) break;
      turn = rows[v.index]!.turn;
    }
    setCurrentTurn(turn);
  }, [isMain, rows, virtualizer, scrollRef]);
  const unlockTurn = useCallback(() => {
    if (isMain && useReading.getState().locked) useReading.setState({ locked: false });
  }, [isMain]);
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (SCROLL_KEYS.has(e.key)) unlockTurn();
    },
    [unlockTurn],
  );

  useEffect(() => {
    if (!isMain) return;
    setTurns(
      rows.flatMap((r) => {
        if (r.kind !== "prompt") return [];
        const o = r.body.origin;
        const text = o.kind === "command" ? commandLine(o.name, o.args) : r.body.text;
        return [{ turn: r.turn, nodeId: r.node.id, text: text.replace(/\s+/g, " ").trim() }];
      }),
    );
  }, [rows, isMain]);
  useEffect(() => (isMain ? resetReading : undefined), [isMain]);

  return { publishTurn, unlockTurn, onKeyDown };
}

/**
 * `follow`: start at the bottom and stay there while content grows if the user was there (`pinned`); a new
 * revision while the user reads elsewhere sets `unread` (the 「有新消息」 button) instead of moving the view.
 */
export function useFollowBottom(
  follow: boolean,
  scrollRef: RefObject<HTMLDivElement | null>,
  virtualizer: ListVirtualizer,
  publishTurn: () => void,
  revision: Transcript["revision"],
) {
  const pinned = useRef(follow);
  const [unread, setUnread] = useState(false);

  const toBottom = useCallback(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [scrollRef]);

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    publishTurn();
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= PIN_THRESHOLD;
    pinned.current = follow && atBottom;
    if (atBottom) setUnread(false);
  }, [follow, publishTurn, scrollRef]);

  // Stay at the bottom when the connection's line under the last message (progress, last turn) grows the content,
  // or the view itself gets shorter (the input area grows: a queued message, a card).
  const tailRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const ro = new ResizeObserver(() => {
      if (pinned.current) toBottom();
    });
    for (const el of [tailRef.current, scrollRef.current]) if (el) ro.observe(el);
    return () => ro.disconnect();
  }, [toBottom, scrollRef]);

  // Stay at the bottom while content grows (new rows, late measurements) if the user was there.
  const total = virtualizer.getTotalSize();
  useLayoutEffect(() => {
    if (pinned.current) toBottom();
    publishTurn();
  }, [total, toBottom, publishTurn]);

  const firstRevision = useRef(true);
  useEffect(() => {
    if (firstRevision.current) {
      firstRevision.current = false;
      return;
    }
    if (follow && !pinned.current) setUnread(true);
  }, [revision, follow]);

  const jumpToBottom = useCallback(() => {
    pinned.current = true;
    toBottom();
    setUnread(false);
  }, [toBottom]);
  // A message sent from the input box: to the end, as a chat app does; again on the next frame, past what the send
  // adds at the end (its placeholder), which would otherwise leave it short and unpinned.
  useEffect(
    () =>
      follow
        ? onScrollToBottom(() => {
            jumpToBottom();
            requestAnimationFrame(jumpToBottom);
          })
        : undefined,
    [follow, jumpToBottom],
  );

  return { pinned, unread, onScroll, tailRef, total, jumpToBottom };
}

/**
 * Consumes the scroll requests for `scope` (pendingJump, find, card ↔ notification links): once the node is in the
 * data, opens what hides it, scrolls it to the center and highlights it for `flashMs`. A missing node is waited for
 * (`resolveScrollTarget`), at most `SCROLL_RETRY_MS` — also when the list unmounts meanwhile, so a later mount
 * does not jump to it long after. The highlight's timer stops with the list.
 */
export function useScrollRequestConsumer({
  scope,
  ready,
  grouping,
  input,
  inheritedExpanded,
  revision,
  virtualizer,
  scrollRef,
  pinned,
}: {
  scope: TranscriptScope;
  ready: boolean;
  grouping: Grouping;
  input: GroupingInput;
  inheritedExpanded: boolean;
  revision: Transcript["revision"];
  virtualizer: ListVirtualizer;
  scrollRef: RefObject<HTMLDivElement | null>;
  pinned: RefObject<boolean>;
}) {
  const pending = usePendingScroll(scope);
  const [target, setTarget] = useState<PendingScroll | null>(null);
  const miss = useRef<ScrollMiss | null>(null);
  const flash = useRef<{ timer: ReturnType<typeof setTimeout>; clear: () => void } | null>(null);

  useEffect(
    () => () => {
      if (flash.current) {
        clearTimeout(flash.current.timer);
        flash.current.clear();
        flash.current = null;
      }
    },
    [],
  );

  useEffect(() => {
    if (!pending || !ready) return;
    const full = inheritedExpanded ? grouping : buildRows({ ...input, inheritedExpanded: true });
    const r = resolveScrollTarget(full, pending, revision, miss.current);
    if (r.kind !== "found") {
      miss.current = r.miss;
      if (r.kind === "drop") clearScroll(scope, pending.seq);
      else if (r.first) {
        const seqNo = pending.seq;
        // Not cleared on unmount: `clearScroll` only drops that request (seq), a late call is harmless.
        setTimeout(() => clearScroll(scope, seqNo), SCROLL_RETRY_MS);
      }
      return;
    }
    miss.current = null;
    clearScroll(scope, pending.seq);
    if (r.expand.length) {
      useUi.setState((s) => ({ expanded: { ...s.expanded, ...Object.fromEntries(r.expand.map((k) => [k, true])) } }));
    }
    pinned.current = false;
    setTarget(pending);
  }, [pending, ready, scope, grouping, input, inheritedExpanded, revision, pinned]);

  useEffect(() => {
    if (!target) return;
    // Wait until the inherited prefix is expanded (the node maps to the banner until then).
    if (grouping.inheritedIds.has(target.nodeId) && !inheritedExpanded) return;
    const idx = rowIndexOf(grouping, target.nodeId, target.toolUseId);
    setTarget(null);
    if (idx === undefined) return;
    virtualizer.scrollToIndex(idx, { align: "center" });
    const el = scrollRef.current;
    if (el) settle(el, anchorKey(target.nodeId, target.toolUseId), target.nodeId);
    const h = { nodeId: target.nodeId, ...(target.toolUseId ? { toolUseId: target.toolUseId } : {}) };
    useUi.getState().setHighlight(h);
    const ms = target.flashMs === undefined ? 2000 : target.flashMs;
    if (ms != null) {
      const clear = () => {
        if (useUi.getState().highlight === h) useUi.getState().setHighlight(null);
      };
      if (flash.current) clearTimeout(flash.current.timer);
      flash.current = {
        timer: setTimeout(() => {
          flash.current = null;
          clear();
        }, ms),
        clear,
      };
    }
  }, [target, grouping, inheritedExpanded, virtualizer, scrollRef]);
}

/**
 * `scrollToIndex` lands on estimated offsets; once the target row is measured, nudge the scroller until the
 * anchor element sits centered (or 24px from the top when taller than the viewport).
 */
function settle(scroller: HTMLElement, anchor: string, nodeId: string) {
  let frames = 0;
  let stable = 0;
  const step = () => {
    const el =
      scroller.querySelector<HTMLElement>(`[data-anchor="${CSS.escape(anchor)}"]`) ??
      scroller.querySelector<HTMLElement>(`[data-anchor="${CSS.escape(nodeId)}"]`);
    if (el) {
      const sr = scroller.getBoundingClientRect();
      const er = el.getBoundingClientRect();
      const want = er.height < sr.height - 48 ? (sr.height - er.height) / 2 : 24;
      const delta = er.top - sr.top - want;
      const before = scroller.scrollTop;
      if (Math.abs(delta) > 2) scroller.scrollTop = before + delta;
      stable = Math.abs(delta) <= 2 || scroller.scrollTop === before ? stable + 1 : 0;
    }
    if (++frames < 40 && stable < 3) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}
