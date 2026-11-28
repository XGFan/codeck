import { X } from "lucide-react";
import { Fragment, useMemo } from "react";
import type { SubagentRun, TranscriptScope } from "@/ipc/bindings";
import { formatDuration, formatTokens } from "@/lib/format";
import { useHotkey } from "@/lib/hotkeys";
import { useCurrentTranscript, useSession, useTranscript } from "@/queries";
import { MAIN_SCOPE, parseWorkerPanelId, selectOverlayOpen, useUi } from "@/state/ui";
import { TranscriptList } from "@/features/transcript/Transcript";
import { RunStatus, useSessionLive } from "@/features/transcript/nodes/Agents";
import { runState } from "@/features/transcript/runState";
import { useHaa } from "@/features/haa/store";
import { isActiveWorker, shortModelName, workerBackend } from "@/features/haa/view";

/** How often a running haa Worker's conversation is re-read (the index does not follow its file). */
const WORKER_FILE_REFETCH_MS = 2000;

const crumbLabel = (run: SubagentRun | undefined, id: string) =>
  run ? [run.agentType ?? run.name, run.description].filter(Boolean).join(" · ") || id : id;

/** Right-side panel showing either the innermost Subagent Run or a haa Worker conversation from `panelStack`. */
export function SubagentPanel() {
  const stack = useUi((s) => s.panelStack);
  const sessionId = useUi((s) => s.sessionId);
  if (!sessionId || stack.length === 0) return null;
  const workerSessionId = parseWorkerPanelId(stack[stack.length - 1]);
  if (workerSessionId) return <WorkerPanel ownerSessionId={sessionId} workerSessionId={workerSessionId} />;
  return <Panel sessionId={sessionId} stack={stack} />;
}

function WorkerPanel({ ownerSessionId, workerSessionId }: { ownerSessionId: string; workerSessionId: string }) {
  const setPanelStack = useUi((s) => s.setPanelStack);
  const includeHidden = useUi((s) => s.showHidden);
  const { data: ownerDetail } = useSession(ownerSessionId);
  const haaWorker = useHaa((s) => s.workers.find((w) => w.sessionId === workerSessionId));
  const tasks = useMemo(
    () => ownerDetail?.haaTasks.filter((t) => t.sessionId === workerSessionId) ?? [],
    [ownerDetail, workerSessionId],
  );
  const req = useMemo(
    () => ({ sessionId: workerSessionId, scope: MAIN_SCOPE, branchChoices: [], includeHidden }),
    [workerSessionId, includeHidden],
  );
  const following = haaWorker != null && isActiveWorker(haaWorker);
  const { data, isPlaceholderData, error } = useTranscript(req, following ? WORKER_FILE_REFETCH_MS : undefined);

  const lastTask = tasks.at(-1);
  const workerName = tasks[0]?.worker ?? haaWorker?.name;
  const agent = workerBackend(haaWorker?.model) ?? lastTask?.backend;
  const model = shortModelName(haaWorker?.model ?? lastTask?.model ?? undefined);
  const modelLabel = [agent, model].filter(Boolean).join(" · ");
  const close = () => setPanelStack([]);

  useHotkey("Escape", () => {
    const s = useUi.getState();
    if (selectOverlayOpen(s) || s.findOpen) return;
    close();
  });

  const loaded = data && data.sessionId === workerSessionId && data.scope.kind === "main";
  const taskCount = Math.max(haaWorker?.tasks ?? 0, tasks.length);
  const taskMs = tasks.reduce((sum, t) => sum + (t.durationMs ?? 0), 0);
  const meta = [`${taskCount} 个 Task`, formatDuration(taskMs > 0 ? taskMs : null, "detail"), "Cost 未记录"].filter(
    Boolean,
  );

  return (
    <aside
      data-testid="worker-panel"
      aria-label="haa Worker 完整过程"
      className="flex w-[min(560px,50%)] min-w-[360px] shrink-0 flex-col border-l border-border bg-list shadow-[-8px_0_24px_rgba(0,0,0,0.04)]"
    >
      <div className="flex shrink-0 flex-col gap-1.5 border-b border-border px-4 py-2.5">
        <nav
          aria-label="面包屑"
          data-testid="panel-breadcrumb"
          className="flex flex-wrap items-center gap-1.5 text-[12px]"
        >
          <button type="button" onClick={close} className="text-accent hover:underline">
            主对话
          </button>
          <span className="text-secondary/70">›</span>
          <span data-testid="crumb" aria-current="page" className="font-semibold">
            {workerName ? `haa Worker · ${workerName}` : "haa Worker"}
          </span>
          <span className="flex-1" />
          <button
            type="button"
            aria-label="关闭面板"
            data-testid="panel-close"
            onClick={close}
            className="flex size-[26px] items-center justify-center rounded-md text-text/80 hover:bg-selection"
          >
            <X size={12} strokeWidth={1.8} aria-hidden />
          </button>
        </nav>
        <div data-testid="panel-meta" className="flex flex-wrap gap-x-3 gap-y-0.5 text-[12px] text-secondary">
          {modelLabel && <span className="text-agy">{modelLabel}</span>}
          {meta.map((m) => (
            <span key={m}>{m}</span>
          ))}
        </div>
      </div>
      {error && !loaded ? (
        <div className="flex flex-1 items-center justify-center px-6 text-[12px] text-secondary">
          无法读取该 haa Worker 的记录
        </div>
      ) : loaded ? (
        <TranscriptList
          key={workerSessionId}
          transcript={data}
          ready={!isPlaceholderData}
          scope={MAIN_SCOPE}
          inPanel
          testId="panel-transcript"
        />
      ) : (
        <div className="flex-1" />
      )}
    </aside>
  );
}

function Panel({ sessionId, stack }: { sessionId: string; stack: string[] }) {
  const agentId = stack[stack.length - 1]!;
  const setPanelStack = useUi((s) => s.setPanelStack);
  const branchChoices = useUi((s) => s.branchChoices);
  const includeHidden = useUi((s) => s.showHidden);
  const scope = useMemo<TranscriptScope>(() => ({ kind: "subagent", agentId }), [agentId]);
  const { data: main } = useCurrentTranscript();
  const { data, isPlaceholderData, error } = useTranscript({ sessionId, scope, branchChoices, includeHidden });

  const runs = useMemo(() => {
    const m = new Map<string, SubagentRun>();
    for (const r of main?.subagents ?? []) m.set(r.agentId, r);
    for (const r of data?.subagents ?? []) m.set(r.agentId, r);
    return m;
  }, [main?.subagents, data?.subagents]);
  // How the run went is read from the file that launched it: its parent's transcript (cached, the panel showed it).
  const parentId = stack.length > 1 ? stack[stack.length - 2]! : null;
  const parentScope = useMemo<TranscriptScope>(() => ({ kind: "subagent", agentId: parentId ?? "" }), [parentId]);
  const { data: parent } = useTranscript(
    parentId ? { sessionId, scope: parentScope, branchChoices, includeHidden } : null,
  );
  const run = (parentId ? parent?.subagents.find((r) => r.agentId === agentId) : undefined) ?? runs.get(agentId);
  const live = useSessionLive(sessionId);
  // A nested run's entry in main knows only its meta: no status until the parent's transcript is in.
  const state = run && (!parentId || parent) ? runState(run, undefined, live) : null;
  const close = () => setPanelStack([]);

  useHotkey("Escape", () => {
    const s = useUi.getState();
    if (selectOverlayOpen(s) || s.findOpen) return;
    close();
  });

  const loaded =
    data && data.sessionId === sessionId && data.scope.kind === "subagent" && data.scope.agentId === agentId;
  const dur = run?.startedMs != null && run.endedMs != null ? run.endedMs - run.startedMs : null;
  const meta = run
    ? [
        run.agentType,
        run.model,
        `${run.messageCount} 条消息`,
        formatDuration(dur, "detail"),
        `输出 ${formatTokens(run.tokens.output)}`,
        `嵌套深度 ${stack.length}`,
        run.isAsync ? "后台" : null,
      ]
    : [`嵌套深度 ${stack.length}`];

  return (
    <aside
      data-testid="subagent-panel"
      aria-label="Subagent 完整过程"
      className="flex w-[min(560px,50%)] min-w-[360px] shrink-0 flex-col border-l border-border bg-list shadow-[-8px_0_24px_rgba(0,0,0,0.04)]"
    >
      <div className="flex shrink-0 flex-col gap-1.5 border-b border-border px-4 py-2.5">
        <nav
          aria-label="面包屑"
          data-testid="panel-breadcrumb"
          className="flex flex-wrap items-center gap-1.5 text-[12px]"
        >
          <button type="button" onClick={close} className="text-accent hover:underline">
            主对话
          </button>
          {stack.map((id, i) => (
            <Fragment key={id}>
              <span className="text-secondary/70">›</span>
              {i === stack.length - 1 ? (
                <span data-testid="crumb" aria-current="page" className="font-semibold">
                  {crumbLabel(runs.get(id), id)}
                </span>
              ) : (
                <button
                  type="button"
                  data-testid="crumb"
                  onClick={() => setPanelStack(stack.slice(0, i + 1))}
                  className="text-accent hover:underline"
                >
                  {crumbLabel(runs.get(id), id)}
                </button>
              )}
            </Fragment>
          ))}
          <span className="flex-1" />
          <button
            type="button"
            aria-label="关闭面板"
            data-testid="panel-close"
            onClick={close}
            className="flex size-[26px] items-center justify-center rounded-md text-text/80 hover:bg-selection"
          >
            <X size={12} strokeWidth={1.8} aria-hidden />
          </button>
        </nav>
        <div data-testid="panel-meta" className="flex flex-wrap gap-x-3 gap-y-0.5 text-[12px] text-secondary">
          {meta.filter(Boolean).map((m) => (
            <span key={m}>{m}</span>
          ))}
          <RunStatus state={state} />
        </div>
      </div>
      {error && !loaded ? (
        <div className="flex flex-1 items-center justify-center px-6 text-[12px] text-secondary">
          无法读取该 Subagent 的记录
        </div>
      ) : loaded ? (
        <TranscriptList
          key={agentId}
          transcript={data}
          ready={!isPlaceholderData}
          scope={scope}
          agent={run ?? null}
          extraRuns={main?.subagents}
          testId="panel-transcript"
        />
      ) : (
        <div className="flex-1" />
      )}
    </aside>
  );
}
