import { Check, Loader, X } from "lucide-react";
import { useMemo, useRef } from "react";
import { buildRows, findCall, RowList, useFollow } from "@/features/transcript";
import type { Block, WorkflowEvent } from "@/protocol";
import { useStore } from "@/state/store";
import { useUi } from "@/state/ui";

const FAILED = new Set(["failed", "error", "killed", "cancelled"]);
const DONE = new Set(["completed", "success", "succeeded", "done"]);
const NO_BLOCKS: Block[] = [];

/** Right-side panel: the Subagent Run / Workflow Run under the Agent / Workflow call in `useUi.panel`. */
export function SubagentPanel() {
  const panel = useUi((s) => s.panel);
  if (!panel) return null;
  return <Panel key={`${panel.conn}:${panel.toolUseId}`} conn={panel.conn} toolUseId={panel.toolUseId} />;
}

function Panel({ conn, toolUseId }: { conn: string; toolUseId: string }) {
  const blocks = useStore((s) => s.conns[conn]?.sub[toolUseId] ?? NO_BLOCKS);
  const items = useStore((s) => s.conns[conn]?.items);
  const sub = useStore((s) => s.conns[conn]?.sub);
  const workflow = useStore((s) => {
    const w = s.conns[conn]?.workflows;
    return w ? Object.values(w).find((x) => x.tool_use_id === toolUseId) : undefined;
  });
  const call = useMemo(() => findCall(items ?? [], sub ?? {}, toolUseId), [items, sub, toolUseId]);
  // Once the Agent / Workflow call has its result, calls inside that never got one will not.
  const finished = call?.result != null;

  const groups = useMemo(() => {
    if (!workflow) return [{ id: "", label: null as string | null, rows: buildRows(blocks, undefined, finished) }];
    const by = new Map<string, Block[]>();
    for (const b of blocks) {
      const k = b.agent_id ?? "";
      by.set(k, [...(by.get(k) ?? []), b]);
    }
    return [...by.entries()].map(([id, bs]) => ({
      id,
      label: id ? (workflow.agents.find((a) => a.agent_id === id)?.label ?? id) : "Workflow",
      rows: buildRows(bs, undefined, finished),
    }));
  }, [blocks, workflow, finished]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const follow = useFollow(scrollRef, contentRef);

  const input = call?.input ?? {};
  const s = (k: string) => (typeof input[k] === "string" ? (input[k] as string) : "");
  const title = workflow
    ? ["Workflow", workflow.name].filter(Boolean).join(" · ")
    : [s("subagent_type") || "Subagent", s("description")].filter(Boolean).join(" · ");
  const status = workflow?.status ?? (call == null ? null : call.result ? (call.result.isError ? "失败" : "完成") : "执行中");

  return (
    <aside
      data-testid="subagent-panel"
      aria-label="Subagent 过程"
      className="flex h-full w-[480px] shrink-0 flex-col border-l border-border bg-list shadow-[-8px_0_24px_rgba(0,0,0,0.04)]"
    >
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2.5">
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] font-semibold">{title}</div>
          <div className="flex gap-3 text-[12px] text-secondary">
            {status && <span>{status}</span>}
            <span>{blocks.length} 块</span>
          </div>
        </div>
        <button
          type="button"
          aria-label="关闭面板"
          data-testid="panel-close"
          onClick={() => useUi.getState().closePanel()}
          className="flex size-[26px] shrink-0 items-center justify-center rounded-md text-text/80 hover:bg-selection"
        >
          <X size={12} strokeWidth={1.8} aria-hidden />
        </button>
      </div>
      <div ref={scrollRef} onScroll={follow.onScroll} className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-4">
        <div ref={contentRef} className="flex flex-col gap-4 py-4">
          {workflow && <Phases run={workflow} />}
          {blocks.length === 0 && <div className="py-6 text-center text-[12px] text-secondary">还没有过程</div>}
          {groups.map((g) => (
            <div key={g.id} data-testid="panel-agent" data-agent-id={g.id || undefined} className="flex flex-col gap-2">
              {g.label && (
                <div className="flex items-center gap-2.5 text-[12px] font-semibold text-secondary">
                  {g.label}
                  <span className="h-px flex-1 bg-border" />
                </div>
              )}
              <RowList conn={conn} rows={g.rows} userLabel="任务" />
            </div>
          ))}
        </div>
      </div>
    </aside>
  );
}

function StateIcon({ state }: { state: string }) {
  if (FAILED.has(state)) return <X size={12} strokeWidth={2.2} className="shrink-0 text-error" aria-label="失败" />;
  if (DONE.has(state)) return <Check size={12} strokeWidth={2.2} className="shrink-0 text-live" aria-label="完成" />;
  return <Loader size={12} strokeWidth={2} className="shrink-0 animate-spin text-secondary" aria-label="运行中" />;
}

/** Workflow phases with their agents (✓ / ✕ / running). */
function Phases({ run }: { run: WorkflowEvent }) {
  const phases = run.phases.map((p) => ({
    key: `p${p.index}`,
    title: `阶段 ${p.index + 1} · ${p.title}`,
    agents: run.agents.filter((a) => a.phase_index === p.index),
  }));
  const loose = run.agents.filter((a) => a.phase_index == null || !run.phases.some((p) => p.index === a.phase_index));
  if (loose.length) phases.push({ key: "loose", title: run.phases.length ? "其他" : "Agent", agents: loose });
  if (phases.length === 0) return null;
  return (
    <div data-testid="workflow-phases" className="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-2">
      {phases.map((p) => (
        <div key={p.key} className="flex flex-col gap-1 rounded-lg border border-border bg-ground px-2.5 py-2">
          <div className="text-[11px] font-semibold text-secondary">{p.title}</div>
          {p.agents.length === 0 && <div className="text-[12px] text-secondary">—</div>}
          {p.agents.map((a) => (
            <div key={a.agent_id} data-testid="workflow-agent" className="flex items-center gap-1.5 text-[12px]">
              <StateIcon state={a.state} />
              <span className="min-w-0 truncate">{a.label || a.agent_id}</span>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
