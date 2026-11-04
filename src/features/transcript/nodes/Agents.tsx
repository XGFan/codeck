import { ArrowDown, Check, Loader, Network, Unlink, Workflow, X } from "lucide-react";
import type { Node, SubagentRun, ToolCall, WorkflowAgent, WorkflowRun } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatTokens } from "@/lib/format";
import { useSession } from "@/queries";
import { useUi } from "@/state/ui";
import type { AgentCardData } from "../grouping";
import { DONE, FAILED, type RunState, runState } from "../runState";
import { clock, elapsed, shortModel } from "../util";
import { Anchor } from "./Anchor";
import { scrollToNode, useTranscriptCtx } from "./scroll";

const STATE_LABEL: Record<RunState, string> = { running: "运行中", done: "已完成", failed: "失败", noNotice: "未收到完成通知" };

/** Whether the Subagent Run has ended: running / done / failed, or a background run whose notification never came. */
export function RunStatus({ state }: { state: RunState | null }) {
  if (!state) return null;
  return (
    <span
      data-testid="run-status"
      data-state={state}
      className={cn(
        "flex shrink-0 items-center gap-1 text-[12px]",
        state === "done" ? "text-live" : state === "failed" ? "text-error" : "text-secondary",
      )}
    >
      {state === "running" && <Loader size={12} strokeWidth={2} className="animate-spin" aria-hidden />}
      {state === "done" && <Check size={12} strokeWidth={2.2} aria-hidden />}
      {state === "failed" && <X size={12} strokeWidth={2.2} aria-hidden />}
      {STATE_LABEL[state]}
    </span>
  );
}

/** Whether the Session is live (its process runs, or it was just written). */
export function useSessionLive(sessionId: string | null): boolean {
  return useSession(sessionId).data?.summary.live != null;
}

const runDuration = (r: SubagentRun) => (r.startedMs != null && r.endedMs != null ? r.endedMs - r.startedMs : null);

function promptOf(run: SubagentRun, call?: ToolCall): string | null {
  if (run.promptPreview) return run.promptPreview;
  if (!call) return null;
  try {
    const v = JSON.parse(call.inputJson) as { prompt?: unknown };
    return typeof v.prompt === "string" ? v.prompt : null;
  } catch {
    return null;
  }
}

/** Subagent Run card: type, description, model/messages/duration, task and result previews, open link. */
export function SubagentCard({ run, call, node }: { run: SubagentRun; call?: ToolCall; node?: Node }) {
  const { sessionId, scope, openAgent, activeAgentId } = useTranscriptCtx();
  const state = runState(run, call, useSessionLive(sessionId));
  const active = activeAgentId === run.agentId;
  const nested = scope.kind === "subagent";
  const failed = state === "failed";
  const prompt = promptOf(run, call);
  const result = run.finalText ?? (call?.result && !run.isAsync ? call.result.text : null);
  const dur = runDuration(run);
  const meta = nested
    ? [`嵌套 · ${run.messageCount} 条`, elapsed(dur)]
    : [shortModel(run.model), `${run.messageCount} 条`, elapsed(dur)];
  const body = (
    <div
      data-testid="subagent-card"
      data-agent-id={run.agentId}
      data-active={active || undefined}
      className={cn(
        "flex w-full flex-col gap-1.5 rounded-[10px] border px-3 py-2.5",
        active
          ? "border-accent bg-selection/40 shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-accent)_12%,transparent)]"
          : "border-border bg-ground",
      )}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Network size={14} strokeWidth={1.5} className={active ? "text-accent" : "text-text/80"} aria-hidden />
        <span className="font-semibold">{run.agentType ?? run.name ?? "Subagent"}</span>
        {run.isAsync && (
          <span className="rounded border border-border px-1 text-[11px] leading-[16px] text-secondary">后台</span>
        )}
        <span className="min-w-[120px] flex-1 truncate">{run.description ?? run.name ?? run.agentId}</span>
        <span className="text-[12px] text-secondary">{meta.filter(Boolean).join(" · ")}</span>
        <RunStatus state={state} />
      </div>
      {prompt && (
        <div className="line-clamp-2 text-[12px] text-text/80">
          <span className="text-secondary">任务</span>
          <span className="ml-3">{prompt}</span>
        </div>
      )}
      {result && (
        <div className="line-clamp-2 text-[12px] text-text/80">
          <span className={failed ? "text-error" : "text-live"}>{failed ? "失败" : "结果"}</span>
          <span className="ml-3">{result}</span>
        </div>
      )}
      <div className="flex items-center gap-3 text-[12px]">
        {run.isAsync && call?.notificationNodeId && (
          <button
            type="button"
            data-testid="card-to-notification"
            onClick={() => scrollToNode({ scope, nodeId: call.notificationNodeId! })}
            className="flex items-center gap-0.5 text-accent hover:underline"
          >
            跳到结果通知
            <ArrowDown size={12} strokeWidth={2} aria-hidden />
          </button>
        )}
        <span className="flex-1" />
        {active ? (
          <span className="font-semibold text-accent">正在右侧查看</span>
        ) : (
          <button
            type="button"
            data-testid="open-subagent"
            onClick={() => openAgent(run.agentId)}
            className="font-semibold text-accent hover:underline"
          >
            {nested ? "进入 →" : "查看完整过程 →"}
          </button>
        )}
      </div>
    </div>
  );
  return node && call ? (
    <Anchor nodeId={node.id} toolUseId={call.toolUseId} className="w-full rounded-[10px]">
      {body}
    </Anchor>
  ) : (
    body
  );
}

export function ParallelAgents({ cards }: { cards: AgentCardData[] }) {
  return (
    <div data-testid="parallel-agents" className="flex w-full flex-col gap-2 border-l-2 border-border pl-3">
      <div className="text-[11px] text-secondary">并行 · {cards.length} 个 Subagent</div>
      {cards.map((c) => (
        <SubagentCard key={c.call.toolUseId} run={c.run} call={c.call} node={c.node} />
      ))}
    </div>
  );
}

function stateIcon(state: string | null) {
  if (FAILED.has(state ?? "")) return <X size={12} strokeWidth={2.2} className="shrink-0 text-error" aria-label="失败" />;
  if (DONE.has(state ?? "")) return <Check size={12} strokeWidth={2.2} className="shrink-0 text-live" aria-label="完成" />;
  return <Loader size={12} strokeWidth={2} className="shrink-0 animate-spin text-secondary" aria-label="运行中" />;
}

/** Workflow Run card: phases with their agents (state ✓ / ✕ / running) and totals; agents open in the panel. */
export function WorkflowCard({ run, node, call }: { run: WorkflowRun; node: Node; call: ToolCall }) {
  const { openAgent, activeAgentId } = useTranscriptCtx();
  const buckets = new Map<number | null, WorkflowAgent[]>();
  for (const a of run.agents) {
    const k = a.phaseIndex ?? null;
    buckets.set(k, [...(buckets.get(k) ?? []), a]);
  }
  const phases: Array<{ key: string; title: string; agents: WorkflowAgent[] }> = run.phases.map((p) => ({
    key: `p${p.index}`,
    title: `阶段 ${p.index + 1} · ${p.title}`,
    agents: buckets.get(p.index) ?? [],
  }));
  const loose = run.agents.filter((a) => a.phaseIndex == null || !run.phases.some((p) => p.index === a.phaseIndex));
  if (loose.length) phases.push({ key: "loose", title: run.phases.length ? "其他" : "Agent", agents: loose });
  const totals = [
    run.phases.length ? `${run.phases.length} 阶段` : null,
    `${run.agents.length} 个 agent`,
    run.totalTokens != null ? `${formatTokens(run.totalTokens)} tokens` : null,
    elapsed(run.durationMs),
  ];
  const failed = FAILED.has(run.status ?? "");

  return (
    <Anchor nodeId={node.id} toolUseId={call.toolUseId} className="w-full rounded-[10px]">
      <div data-testid="workflow-card" className="flex w-full flex-col gap-2 rounded-[10px] border border-border px-3 py-2.5">
        <div className="flex flex-wrap items-center gap-2">
          <Workflow size={14} strokeWidth={1.5} className="text-text/80" aria-hidden />
          <span className="font-semibold">Workflow</span>
          <span className="min-w-[120px] flex-1 truncate">
            {[run.name, run.summary].filter(Boolean).join(" · ") || run.runId}
          </span>
          {failed && <span className="text-[12px] font-semibold text-error">失败</span>}
          <span className="text-[12px] text-secondary">{totals.filter(Boolean).join(" · ")}</span>
        </div>
        {phases.length > 0 && (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-2">
            {phases.map((p) => (
              <div key={p.key} className="flex flex-col gap-1 rounded-lg bg-list px-2.5 py-2">
                <div className="text-[11px] font-semibold text-secondary">{p.title}</div>
                {p.agents.length === 0 && <div className="text-[12px] text-secondary">—</div>}
                {p.agents.map((a) => (
                  <button
                    key={a.agentId}
                    type="button"
                    data-testid="workflow-agent"
                    title={a.resultPreview ?? undefined}
                    onClick={() => openAgent(a.agentId)}
                    className={cn(
                      "flex items-center gap-1.5 rounded px-1 -mx-1 text-left text-[12px] hover:bg-selection/60",
                      activeAgentId === a.agentId && "bg-selection font-semibold",
                    )}
                  >
                    {stateIcon(a.state)}
                    <span className="min-w-0 truncate">{a.label ?? a.agentId}</span>
                  </button>
                ))}
              </div>
            ))}
          </div>
        )}
      </div>
    </Anchor>
  );
}

export function OrphanHeader({ count }: { count: number }) {
  return (
    <div className="flex items-center gap-2.5 pt-2 text-[12px] text-secondary">
      <Unlink size={13} strokeWidth={1.5} aria-hidden />
      <span>未关联的 Subagent · {count}</span>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}

/** Subagent scope: the task prompt the parent sent (or a later message from it), clamped with "展开全部". */
export function TaskPromptCard({ node, text, title = "任务 PROMPT" }: { node: Node; text: string; title?: string }) {
  const key = `taskPrompt:${node.id}`;
  const open = useUi((s) => s.expanded[key] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const long = text.length > 240 || text.split("\n").length > 4;
  return (
    <Anchor nodeId={node.id}>
      <div data-testid="task-prompt" className="rounded-lg border border-border bg-ground px-3 py-2.5">
        <div className="mb-1 flex items-center gap-2 text-[11px] font-semibold text-secondary">
          {title}<span className="font-normal">{clock(node.timestampMs)}</span>
        </div>
        <div className={cn("text-[12px] break-words whitespace-pre-wrap text-text/80", !open && long && "line-clamp-4")}>{text}</div>
        {long && (
          <button type="button" onClick={() => toggle(key)} className="mt-1 text-[12px] text-accent hover:underline">
            {open ? "收起" : "展开全部"}
          </button>
        )}
      </div>
    </Anchor>
  );
}

export function FinalResultCard({ text }: { text: string }) {
  return (
    <div data-testid="final-result" className="rounded-lg border border-live/25 bg-live/5 px-3 py-2.5">
      <div className="mb-1 text-[11px] font-semibold text-live">最终结果 · 返回给主对话</div>
      <div className="text-[12px] break-words whitespace-pre-wrap">{text}</div>
    </div>
  );
}
