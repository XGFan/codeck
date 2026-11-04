import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { SubagentRun, Transcript, TranscriptScope } from "@/ipc/bindings";
import { ConnectTail, useUnindexed } from "@/features/connect";
import { cn } from "@/lib/cn";
import { useCurrentTranscript } from "@/queries";
import { CONV_WIDTH, useUi } from "@/state/ui";
import { foldTurns } from "./fold";
import { anchorKey, buildRows, type GroupingInput, INHERITED_KEY, locate, rowIndexOf } from "./grouping";
import { isContinuation, RowView } from "./nodes";
import { clearScroll, type PendingScroll, scrollToNode, TranscriptContext, type TranscriptCtx, usePendingScroll } from "./nodes/scroll";
import { resetReading, setCurrentTurn, setTurns, useReading } from "./reading";
import { toolKey } from "./tools";

/**
 * Scroll + highlight API for other features (⌘F / T4.2, outline, links):
 *
 * ```ts
 * scrollToNode({ scope, nodeId, toolUseId, thinking: loc.kind === "thinking", flashMs: null });
 * ```
 *
 * The list rendering `scope` (main transcript, or the Subagent panel when `scope.kind === "subagent"` is the
 * panel's innermost agent) expands whatever hides the target — the Fork-inherited prefix (`inherited`), the
 * tool group (`group:<firstNodeId>|<firstToolUseId>`), the call (`tool:<nodeId>|<toolUseId>`), thinking
 * (`thinking:<nodeId>`) or a compact summary (`compact:<nodeId>`) — scrolls the virtualizer so the target is
 * centered, then sets `useUi.highlight` (cleared after `flashMs`, default 2000; `null` keeps it).
 * A request for a scope that is not mounted yet waits for it; one whose node is not in the data yet is retried on
 * the next data update (dropped after two revisions or 5 s).
 */
export { scrollToNode, type ScrollRequest } from "./nodes/scroll";

declare global {
  interface Window {
    /** E2E hook: the UI store, exposed in dev / mock builds only. */
    __cvStore?: typeof useUi;
  }
}
if (import.meta.env.DEV || import.meta.env.VITE_IPC === "mock") window.__cvStore = useUi;

const MAIN: TranscriptScope = { kind: "main" };
const PIN_THRESHOLD = 48;
/** A scroll request whose node is missing waits this long / this many revisions for it to arrive. */
const SCROLL_RETRY_MS = 5000;
const SCROLL_RETRY_REVISIONS = 2;
/** Keys that scroll the focused list; they end the outline jump lock like a wheel or drag does. */
const SCROLL_KEYS = new Set(["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "]);

/** The fold keys' state in `expanded`, as a string (stable while only other keys change). */
const foldState = (expanded: Record<string, boolean>) =>
  Object.keys(expanded)
    .filter((k) => k.startsWith("fold:"))
    .map((k) => `${k}=${expanded[k] ? 1 : 0}`)
    .join(",");

export function TranscriptView() {
  const sessionId = useUi((s) => s.sessionId);
  const { data, isPlaceholderData, error } = useCurrentTranscript();
  // A Connected or `ccrpc ls` Session not in the index yet: its transcript file appears with the first prompt.
  const unindexed = useUnindexed(error);
  usePendingJump();

  if (!sessionId) {
    return (
      <div data-testid="transcript-empty" className="flex flex-1 items-center justify-center text-[13px] text-secondary">
        选择一个 Session 查看对话
      </div>
    );
  }
  if (unindexed && data?.sessionId !== sessionId) {
    return (
      <div data-testid="transcript-new" className="flex flex-1 flex-col items-center justify-center px-6 text-[13px] text-secondary">
        新 Session，发送第一条提示词后显示对话
        <ConnectTail />
      </div>
    );
  }
  if (error && data?.sessionId !== sessionId) {
    const message = typeof error === "object" && "message" in error ? String(error.message) : String(error);
    return (
      <div data-testid="transcript-error" role="alert" className="flex flex-1 items-center justify-center px-6 text-[13px] text-error">
        无法读取该 Session：{message}
      </div>
    );
  }
  // keepPreviousData may still hold the previous session's transcript while the new one loads.
  if (!data || data.sessionId !== sessionId) {
    return <div data-testid="transcript" className="flex-1" />;
  }
  return <TranscriptList key={sessionId} transcript={data} ready={!isPlaceholderData} scope={MAIN} follow testId="transcript" />;
}

/**
 * A2/A4 pendingJump consumer: applies the target's branch choices / hidden flag / panel stack, then queues the
 * scroll in the scope that shows the node (main or the Subagent panel) and clears `pendingJump`.
 */
function usePendingJump() {
  const jump = useUi((s) => s.pendingJump);
  const sessionId = useUi((s) => s.sessionId);
  useEffect(() => {
    if (!jump || jump.sessionId !== sessionId) return;
    const path = [...jump.agentPath];
    if (jump.scope.kind === "subagent" && path[path.length - 1] !== jump.scope.agentId) path.push(jump.scope.agentId);
    useUi.setState((s) => ({
      scope: MAIN,
      branchChoices: jump.branchChoices,
      showHidden: s.showHidden || jump.needsHidden,
      panelStack: path,
      pendingJump: null,
    }));
    scrollToNode({ scope: jump.scope, nodeId: jump.nodeId, toolUseId: jump.toolUseId, flashMs: 2000 });
  }, [jump, sessionId]);
}

export interface TranscriptListProps {
  transcript: Transcript;
  /** False while react-query shows placeholder data for a different request; scroll requests wait for true. */
  ready: boolean;
  scope: TranscriptScope;
  /** Start at the bottom and stay pinned while content grows (main transcript). */
  follow?: boolean;
  /** Subagent scope: the run being shown (task card + final result). */
  agent?: SubagentRun | null;
  /** Runs known from elsewhere (the main transcript) for card / breadcrumb lookups. */
  extraRuns?: SubagentRun[];
  testId?: string;
}

/** Virtualized row list shared by the main transcript and the Subagent panel. */
export function TranscriptList({ transcript, ready, scope, follow = false, agent = null, extraRuns, testId }: TranscriptListProps) {
  const sessionId = transcript.sessionId;
  const showHidden = useUi((s) => s.showHidden);
  const inheritedExpanded = useUi((s) => s.expanded[INHERITED_KEY] ?? false);
  const panelStack = useUi((s) => s.panelStack);
  const setPanelStack = useUi((s) => s.setPanelStack);

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
  const grouping = useMemo(() => foldTurns(built, useUi.getState().expanded), [built, folds]);
  const rows = grouping.rows;


  const ctx = useMemo<TranscriptCtx>(() => {
    const runs = new Map<string, SubagentRun>();
    for (const r of extraRuns ?? []) runs.set(r.agentId, r);
    for (const r of transcript.subagents) runs.set(r.agentId, r);
    return {
      sessionId,
      scope,
      runs,
      toolNode: grouping.toolNode,
      activeAgentId: panelStack[panelStack.length - 1] ?? null,
      openAgent: (agentId) => {
        const stack = useUi.getState().panelStack;
        if (scope.kind === "main") setPanelStack([agentId]);
        else {
          const at = stack.indexOf(scope.agentId);
          setPanelStack([...(at >= 0 ? stack.slice(0, at + 1) : stack), agentId]);
        }
      },
    };
  }, [sessionId, scope, extraRuns, transcript.subagents, grouping.toolNode, panelStack, setPanelStack]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(follow);
  const [unread, setUnread] = useState(false);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 110,
    overscan: 8,
    getItemKey: (i) => rows[i]!.key,
  });

  const toBottom = useCallback(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);

  const isMain = scope.kind === "main";
  // Outline / j-k support (main list only): which turn the reading line (55% down the viewport) is in.
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
  }, [isMain, rows, virtualizer]);
  const unlockTurn = useCallback(() => {
    if (isMain && useReading.getState().locked) useReading.setState({ locked: false });
  }, [isMain]);
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (SCROLL_KEYS.has(e.key)) unlockTurn();
    },
    [unlockTurn],
  );

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    publishTurn();
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= PIN_THRESHOLD;
    pinned.current = follow && atBottom;
    if (atBottom) setUnread(false);
  }, [follow, publishTurn]);

  useEffect(() => {
    if (!isMain) return;
    setTurns(
      rows.flatMap((r) => {
        if (r.kind !== "prompt") return [];
        const o = r.body.origin;
        const text = o.kind === "command" ? `${o.name} ${o.args}`.trim() : r.body.text;
        return [{ turn: r.turn, nodeId: r.node.id, text: text.replace(/\s+/g, " ").trim() }];
      }),
    );
  }, [rows, isMain]);
  useEffect(() => (isMain ? resetReading : undefined), [isMain]);

  // Stay at the bottom when the connection's line under the last message (progress, last turn) grows the content,
  // or the view itself gets shorter (the input area grows: a queued message, a card).
  const tailRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const ro = new ResizeObserver(() => {
      if (pinned.current) toBottom();
    });
    for (const el of [tailRef.current, scrollRef.current]) if (el) ro.observe(el);
    return () => ro.disconnect();
  }, [toBottom]);

  // Stay at the bottom while content grows (new rows, late measurements) if the user was there.
  const total = virtualizer.getTotalSize();
  useLayoutEffect(() => {
    if (pinned.current) toBottom();
    publishTurn();
  }, [total, toBottom, publishTurn]);

  // New revision while the user reads elsewhere: offer a jump instead of moving the view.
  const firstRevision = useRef(true);
  useEffect(() => {
    if (firstRevision.current) {
      firstRevision.current = false;
      return;
    }
    if (follow && !pinned.current) setUnread(true);
  }, [transcript.revision, follow]);

  // ---- scroll requests (pendingJump, find, card ↔ notification links) ----
  const pending = usePendingScroll(scope);
  const [target, setTarget] = useState<PendingScroll | null>(null);
  // The pending request whose node was not found yet: revisions seen since, for the retry bound.
  const miss = useRef<{ seq: number; revision: Transcript["revision"]; changes: number } | null>(null);
  const revision = transcript.revision;

  useEffect(() => {
    if (!pending || !ready) return;
    const full = inheritedExpanded ? grouping : buildRows({ ...input, inheritedExpanded: true });
    const loc = locate(full, pending.nodeId, pending.toolUseId, { thinking: pending.thinking, toolKey });
    if (!loc) {
      // The node may arrive with the next data update (live append, refetch): keep the request for a while.
      const m = miss.current;
      if (m?.seq !== pending.seq) {
        miss.current = { seq: pending.seq, revision, changes: 0 };
        const seqNo = pending.seq;
        setTimeout(() => clearScroll(scope, seqNo), SCROLL_RETRY_MS);
      } else if (m.revision !== revision) {
        m.revision = revision;
        if (++m.changes >= SCROLL_RETRY_REVISIONS) clearScroll(scope, pending.seq);
      }
      return;
    }
    miss.current = null;
    clearScroll(scope, pending.seq);
    if (loc.expand.length) {
      useUi.setState((s) => ({ expanded: { ...s.expanded, ...Object.fromEntries(loc.expand.map((k) => [k, true])) } }));
    }
    pinned.current = false;
    setTarget(pending);
  }, [pending, ready, scope, grouping, input, inheritedExpanded, revision]);

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
    const flash = target.flashMs === undefined ? 2000 : target.flashMs;
    if (flash != null) {
      setTimeout(() => {
        if (useUi.getState().highlight === h) useUi.getState().setHighlight(null);
      }, flash);
    }
  }, [target, grouping, inheritedExpanded, virtualizer]);

  return (
    <TranscriptContext.Provider value={ctx}>
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
        <div
          ref={scrollRef}
          onScroll={onScroll}
          onWheel={unlockTurn}
          onPointerDown={unlockTurn}
          onTouchStart={unlockTurn}
          onKeyDown={onKeyDown}
          data-testid={testId}
          data-scope={scope.kind === "main" ? "main" : scope.agentId}
          className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-6"
        >
          <div className="relative mx-auto max-w-(--conv-w)" style={{ height: total + 40 }}>
            {virtualizer.getVirtualItems().map((v) => {
              const row = rows[v.index]!;
              return (
                <div
                  key={v.key}
                  ref={virtualizer.measureElement}
                  data-index={v.index}
                  data-row={row.kind}
                  className={cn("absolute top-0 left-0 w-full", v.index === 0 ? "" : isContinuation(row) ? "pt-2" : "pt-[18px]")}
                  style={{ transform: `translateY(${v.start + 20}px)` }}
                >
                  <RowView row={row} />
                </div>
              );
            })}
          </div>
          {follow && (
            // Room at the end for the floating input area (its height, kept in `--connect-h` by the panel).
            <div ref={tailRef} className="mx-auto -mt-5 max-w-(--conv-w)" style={{ paddingBottom: "calc(var(--connect-h, 0px) + 20px)" }}>
              <ConnectTail />
            </div>
          )}
        </div>
        {isMain && <WidthHandles />}
        {unread && (
          <button
            type="button"
            data-testid="new-messages"
            onClick={() => {
              pinned.current = true;
              toBottom();
              setUnread(false);
            }}
            style={{ bottom: "calc(var(--connect-h, 0px) + 16px)" }}
            className="absolute left-1/2 z-10 flex -translate-x-1/2 items-center gap-1 rounded-full bg-accent px-3 py-1 text-[12px] text-white shadow-lg"
          >
            有新消息
            <ArrowDown size={12} strokeWidth={2} />
          </button>
        )}
      </div>
    </TranscriptContext.Provider>
  );
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


/**
 * The conversation column's edges, to drag its width (both sides move, it stays centered); double-click goes
 * back to the default; ← / → when focused. Lies over the column as it is laid out (narrower when the pane is).
 */
function WidthHandles() {
  const box = useRef<HTMLDivElement>(null);
  const setWidth = useUi((s) => s.setConvWidth);
  const drag = (side: -1 | 1) => (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !box.current) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const x0 = e.clientX;
    const w0 = box.current.getBoundingClientRect().width;
    const move = (m: PointerEvent) => setWidth(w0 + side * 2 * (m.clientX - x0));
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = "col-resize";
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
  };
  const keys = (side: -1 | 1) => (e: React.KeyboardEvent) => {
    const d = e.key === "ArrowRight" ? 20 : e.key === "ArrowLeft" ? -20 : 0;
    if (!d || !box.current) return;
    e.preventDefault();
    setWidth(box.current.getBoundingClientRect().width + side * 2 * d);
  };
  const handle = (side: -1 | 1) => (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="拖动调节对话宽度，双击恢复默认"
      title="拖动调节对话宽度，双击恢复默认"
      tabIndex={0}
      data-testid={side < 0 ? "width-handle-left" : "width-handle-right"}
      onPointerDown={drag(side)}
      onDoubleClick={() => setWidth(CONV_WIDTH.default)}
      onKeyDown={keys(side)}
      className={cn(
        "group/w pointer-events-auto absolute inset-y-0 w-3 cursor-col-resize outline-none",
        side < 0 ? "-left-4" : "-right-4",
      )}
    >
      <span className="absolute inset-y-0 left-1/2 w-px bg-transparent transition-colors group-hover/w:bg-border group-focus-visible/w:bg-accent" />
    </div>
  );
  return (
    <div
      ref={box}
      data-testid="conversation-column"
      className="pointer-events-none absolute inset-y-0 left-1/2 -translate-x-1/2"
      style={{ width: "min(var(--conv-w), calc(100% - 48px))" }}
    >
      {handle(-1)}
      {handle(1)}
    </div>
  );
}
