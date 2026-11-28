import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown } from "lucide-react";
import { useEffect, useMemo, useRef } from "react";
import type { HaaTask, SubagentRun, Transcript, TranscriptScope } from "@/ipc/bindings";
import { OverlayScrollbar } from "@/components/OverlayScrollbar";
import { ConnectTail, RewoundDivider, useRewindView, useShownTranscript, useUnindexed } from "@/features/connect";
import { cn } from "@/lib/cn";
import { useCurrentTranscript, useIndexStatus, useSession } from "@/queries";
import { MAIN_SCOPE, parseWorkerPanelId, useUi, workerPanelId } from "@/state/ui";
import { useFollowBottom, usePublishTurns, useRows, useScrollRequestConsumer } from "./listHooks";
import { isContinuation, RowView } from "./nodes";
import { scrollToNode, TranscriptContext, type TranscriptCtx } from "./nodes/scroll";
import { peerNames } from "./teammate";
import { ColumnBox } from "./ColumnBox";
import { errorText, isNotFound } from "@/lib/errors";

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
  const { data: detail } = useSession(sessionId);
  // A Connected or `ccrpc ls` Session not in the index yet: its transcript file appears with the first prompt.
  const unindexed = useUnindexed(error);
  // A link may name a Session the scan has not reached yet (just started, or the app launched by the link).
  const scanning = useIndexStatus().data?.phase === "scanning";
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
  if (scanning && isNotFound(error) && data?.sessionId !== sessionId) {
    return (
      <div
        data-testid="transcript-scanning"
        className="flex flex-1 items-center justify-center px-6 text-[13px] text-secondary"
      >
        正在更新索引，完成后显示这个 Session
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
      haaTasks={detail?.summary.id === sessionId ? detail.haaTasks : undefined}
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
  /** Start at the bottom and stay pinned while content grows (main transcript; a working Worker's in the panel). */
  follow?: boolean;
  /** True when rendered inside the right-hand panel (e.g. a haa Worker's main conversation). */
  inPanel?: boolean;
  /** Completed haa Worker Tasks owned by this Session; inserted at top level in the primary main conversation only. */
  haaTasks?: readonly HaaTask[];
  /** Subagent scope: the run being shown (task card + final result). */
  agent?: SubagentRun | null;
  /** Runs known from elsewhere (the main transcript) for card / breadcrumb lookups. */
  extraRuns?: SubagentRun[];
  testId?: string;
}

const NO_PROMPTS: ReadonlySet<string> = new Set();

/** Virtualized row list shared by the main transcript and the Subagent / Worker panel. */
export function TranscriptList({
  transcript: fetched,
  ready,
  scope,
  follow = false,
  inPanel = false,
  haaTasks,
  agent = null,
  extraRuns,
  testId,
}: TranscriptListProps) {
  const isMain = scope.kind === "main" && !inPanel;
  const transcript = useShownTranscript(fetched, isMain);
  const sessionId = transcript.sessionId;
  const panelStack = useUi((s) => s.panelStack);
  const setPanelStack = useUi((s) => s.setPanelStack);
  const { input, grouping, inheritedExpanded } = useRows(transcript, agent, isMain ? haaTasks : undefined);
  const rows = grouping.rows;
  const rewind = useRewindView(isMain ? transcript : null, rows);
  const rewindable = rewind?.rewindable ?? NO_PROMPTS;
  const forkable = rewind?.forkable ?? NO_PROMPTS;
  // The prompts shown: a prompt just sent stays a placeholder at the end until it is among them.
  const promptIds = useMemo(() => new Set(rows.flatMap((r) => (r.kind === "prompt" ? [r.node.id] : []))), [rows]);

  const names = useMemo(() => peerNames(transcript.nodes), [transcript.nodes]);
  const ctx = useMemo<TranscriptCtx>(() => {
    const runs = new Map<string, SubagentRun>();
    for (const r of extraRuns ?? []) runs.set(r.agentId, r);
    for (const r of transcript.subagents) runs.set(r.agentId, r);
    const topPanel = panelStack[panelStack.length - 1] ?? null;
    const activeWorkerId = parseWorkerPanelId(topPanel);
    return {
      sessionId,
      scope,
      runs,
      toolNode: grouping.toolNode,
      activeAgentId: activeWorkerId ? null : topPanel,
      activeWorkerId,
      rewindable,
      forkable,
      peerNames: names,
      openAgent: (agentId) => {
        const stack = useUi.getState().panelStack;
        if (scope.kind === "main") setPanelStack([agentId]);
        else {
          const at = stack.indexOf(scope.agentId);
          setPanelStack([...(at >= 0 ? stack.slice(0, at + 1) : stack), agentId]);
        }
      },
      openWorker: (workerSessionId) => setPanelStack([workerPanelId(workerSessionId)]),
    };
  }, [
    sessionId,
    scope,
    extraRuns,
    transcript.subagents,
    grouping.toolNode,
    panelStack,
    setPanelStack,
    rewindable,
    forkable,
    names,
  ]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 110,
    overscan: 8,
    getItemKey: (i) => rows[i]!.key,
  });

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
    enabled: !(inPanel && scope.kind === "main"),
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
          data-scope={inPanel && scope.kind === "main" ? undefined : scope.kind === "main" ? "main" : scope.agentId}
          className="no-scrollbar min-h-0 flex-1 overflow-y-auto overscroll-contain px-6"
        >
          <div className="relative mx-auto max-w-(--conv-w)" style={{ height: total + 40 }}>
            {virtualizer.getVirtualItems().map((v) => {
              const row = rows[v.index]!;
              // Rewound (conversation restored, no prompt since): dimmed under a divider, until the next prompt.
              const dimFrom = rewind?.dimFrom ?? null;
              const rewound = dimFrom != null && v.index >= dimFrom;
              return (
                <div
                  key={v.key}
                  ref={virtualizer.measureElement}
                  data-index={v.index}
                  data-row={row.kind}
                  data-rewound={rewound || undefined}
                  className={cn(
                    // A preview open in the row (an image marker's, which may hang below it) over the rows after it.
                    "absolute top-0 left-0 w-full has-[[role=tooltip]]:z-10",
                    v.index === 0 ? "" : isContinuation(row) ? "pt-2" : "pt-[18px]",
                    rewound && v.index > dimFrom && "opacity-[.38]",
                  )}
                  style={{ transform: `translateY(${v.start + 20}px)` }}
                >
                  {v.index === dimFrom ? (
                    <>
                      <RewoundDivider label={rewind!.label} />
                      <div className="opacity-[.38]">
                        <RowView row={row} />
                      </div>
                    </>
                  ) : (
                    <RowView row={row} />
                  )}
                </div>
              );
            })}
          </div>
          {follow && !inPanel && (
            // Room at the end for the floating input area (its height, kept in `--connect-h` by the panel), and the
            // open Session's connection line: neither belongs to a Worker conversation followed in the right panel.
            <div
              ref={tailRef}
              className="mx-auto -mt-5 max-w-(--conv-w)"
              style={{ paddingBottom: "calc(var(--connect-h, 0px) + 20px)" }}
            >
              <ConnectTail promptIds={promptIds} />
            </div>
          )}
        </div>
        <OverlayScrollbar scrollRef={scrollRef} onUserScroll={unlockTurn} testId={testId && `${testId}-scrollbar`} />
        {isMain && <ColumnBox />}
        {unread && (
          // Centered by flex, not translate(-50%): an odd width would put the text on a half pixel, which blurs it.
          <div
            style={{ bottom: "calc(var(--connect-h, 0px) + 16px)" }}
            className="pointer-events-none absolute inset-x-0 z-10 flex justify-center"
          >
            <button
              type="button"
              data-testid="new-messages"
              onClick={jumpToBottom}
              className="pointer-events-auto flex items-center gap-1 rounded-full bg-accent px-3 py-1 text-[12px] text-white shadow-lg"
            >
              有新消息
              <ArrowDown size={12} strokeWidth={2} />
            </button>
          </div>
        )}
      </div>
    </TranscriptContext.Provider>
  );
}
