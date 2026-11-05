import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown } from "lucide-react";
import { useEffect, useMemo, useRef } from "react";
import type { SubagentRun, Transcript, TranscriptScope } from "@/ipc/bindings";
import { OverlayScrollbar } from "@/components/OverlayScrollbar";
import { ConnectTail, useUnindexed } from "@/features/connect";
import { cn } from "@/lib/cn";
import { useCurrentTranscript } from "@/queries";
import { MAIN_SCOPE, useUi } from "@/state/ui";
import { useFollowBottom, usePublishTurns, useRows, useScrollRequestConsumer } from "./listHooks";
import { isContinuation, RowView } from "./nodes";
import { scrollToNode, TranscriptContext, type TranscriptCtx } from "./nodes/scroll";
import { WidthHandles } from "./WidthHandles";
import { errorText } from "@/lib/errors";

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
 * the next data update (dropped after two revisions or 5 s, even if the list unmounts meanwhile).
 */
export { scrollToNode, type ScrollRequest } from "./nodes/scroll";

declare global {
  interface Window {
    /** E2E hook: the UI store, exposed in dev / mock builds only. */
    __cvStore?: typeof useUi;
  }
}
if (import.meta.env.DEV || import.meta.env.VITE_IPC === "mock") window.__cvStore = useUi;

export function TranscriptView() {
  const sessionId = useUi((s) => s.sessionId);
  const { data, isPlaceholderData, error } = useCurrentTranscript();
  // A Connected or `ccrpc ls` Session not in the index yet: its transcript file appears with the first prompt.
  const unindexed = useUnindexed(error);
  usePendingJump();

  if (!sessionId) {
    return (
      <div
        data-testid="transcript-empty"
        className="flex flex-1 items-center justify-center text-[13px] text-secondary"
      >
        选择一个 Session 查看对话
      </div>
    );
  }
  if (unindexed && data?.sessionId !== sessionId) {
    return (
      <div
        data-testid="transcript-new"
        className="flex flex-1 flex-col items-center justify-center px-6 text-[13px] text-secondary"
      >
        新 Session，发送第一条提示词后显示对话
        <ConnectTail />
      </div>
    );
  }
  if (error && data?.sessionId !== sessionId) {
    const message = errorText(error);
    return (
      <div
        data-testid="transcript-error"
        role="alert"
        className="flex flex-1 items-center justify-center px-6 text-[13px] text-error"
      >
        无法读取该 Session：{message}
      </div>
    );
  }
  // keepPreviousData may still hold the previous session's transcript while the new one loads.
  if (!data || data.sessionId !== sessionId) {
    return <div data-testid="transcript" className="flex-1" />;
  }
  return (
    <TranscriptList
      key={sessionId}
      transcript={data}
      ready={!isPlaceholderData}
      scope={MAIN_SCOPE}
      follow
      testId="transcript"
    />
  );
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
export function TranscriptList({
  transcript,
  ready,
  scope,
  follow = false,
  agent = null,
  extraRuns,
  testId,
}: TranscriptListProps) {
  const sessionId = transcript.sessionId;
  const panelStack = useUi((s) => s.panelStack);
  const setPanelStack = useUi((s) => s.setPanelStack);
  const { input, grouping, inheritedExpanded } = useRows(transcript, agent);
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
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 110,
    overscan: 8,
    getItemKey: (i) => rows[i]!.key,
  });

  const isMain = scope.kind === "main";
  const { publishTurn, unlockTurn, onKeyDown } = usePublishTurns(isMain, rows, virtualizer, scrollRef);
  const { pinned, unread, onScroll, tailRef, total, jumpToBottom } = useFollowBottom(
    follow,
    scrollRef,
    virtualizer,
    publishTurn,
    transcript.revision,
  );
  useScrollRequestConsumer({
    scope,
    ready,
    grouping,
    input,
    inheritedExpanded,
    revision: transcript.revision,
    virtualizer,
    scrollRef,
    pinned,
  });

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
          className="no-scrollbar min-h-0 flex-1 overflow-y-auto overscroll-contain px-6"
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
                  className={cn(
                    "absolute top-0 left-0 w-full",
                    v.index === 0 ? "" : isContinuation(row) ? "pt-2" : "pt-[18px]",
                  )}
                  style={{ transform: `translateY(${v.start + 20}px)` }}
                >
                  <RowView row={row} />
                </div>
              );
            })}
          </div>
          {follow && (
            // Room at the end for the floating input area (its height, kept in `--connect-h` by the panel).
            <div
              ref={tailRef}
              className="mx-auto -mt-5 max-w-(--conv-w)"
              style={{ paddingBottom: "calc(var(--connect-h, 0px) + 20px)" }}
            >
              <ConnectTail />
            </div>
          )}
        </div>
        <OverlayScrollbar scrollRef={scrollRef} onUserScroll={unlockTurn} testId={testId && `${testId}-scrollbar`} />
        {isMain && <WidthHandles />}
        {unread && (
          <button
            type="button"
            data-testid="new-messages"
            onClick={jumpToBottom}
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
