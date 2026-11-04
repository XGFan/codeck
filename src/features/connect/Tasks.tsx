import { Layers, Square } from "lucide-react";
import { Popover } from "radix-ui";
import { footBtn } from "./Header";
import { useState } from "react";
import { useCurrentTranscript } from "@/queries";
import { useUi } from "@/state/ui";
import type { ConnState } from "./conn";
import { errorKey, useConnect } from "./store";

const KIND: Record<string, string> = { agent: "Subagent", shell: "Shell", workflow: "Workflow" };

/**
 * Background tasks: the running ones' count beside the input box (nothing when none runs), the list (with stop)
 * in a popover; finished ones only on request. A Subagent opens its Subagent Run in the side panel.
 */
export function Tasks({ c }: { c: ConnState }) {
  const command = useConnect((s) => s.command);
  const refresh = useConnect((s) => s.refreshTasks);
  const setError = useConnect((s) => s.setError);
  const [showDone, setShowDone] = useState(false);
  const openSession = useUi((s) => s.sessionId);
  const setPanelStack = useUi((s) => s.setPanelStack);
  const { data: transcript } = useCurrentTranscript();
  const runs = new Map(c.sessionId === openSession ? transcript?.subagents.map((r) => [r.agentId, r]) : []);
  // A nested run opens under its parents, as its card's 「进入」 would.
  const stackOf = (agentId: string) => {
    const stack: string[] = [];
    for (let id: string | null | undefined = agentId; id && !stack.includes(id); id = runs.get(id)?.parentAgentId) stack.unshift(id);
    return stack;
  };
  const running = c.tasks.filter((t) => t.status === "running");
  const done = c.tasks.length - running.length;
  if (running.length === 0) return null;
  const shown = showDone ? c.tasks : running;
  return (
    <Popover.Root onOpenChange={(o) => o && void refresh(c.conn)}>
      <Popover.Trigger className={footBtn} data-testid="tasks">
        <Layers size={14} strokeWidth={1.7} aria-hidden /> {running.length} 个后台任务
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="end" sideOffset={4} className="z-50 w-96 rounded-md border border-border bg-ground p-2 shadow-lg">
          <ul className="max-h-80 space-y-1 overflow-y-auto" data-testid="tasks-list">
            {shown.map((t) => (
              <li key={t.id} className="flex items-center gap-2 rounded px-2 py-1 text-[12px]">
                <span className="shrink-0 text-secondary">{KIND[t.kind] ?? t.kind}</span>
                {t.kind === "agent" && runs.has(t.id) ? (
                  <Popover.Close
                    data-testid="task-open"
                    onClick={() => setPanelStack(stackOf(t.id))}
                    title="在右侧查看完整过程"
                    className="min-w-0 flex-1 truncate text-left text-accent hover:underline"
                  >
                    {t.description || t.id}
                  </Popover.Close>
                ) : (
                  <span className="min-w-0 flex-1 truncate" title={t.command ?? t.description ?? ""}>
                    {t.description || t.command || t.id}
                  </span>
                )}
                <span className="shrink-0 text-secondary">{t.status}</span>
                {t.status === "running" && (
                  <button
                    type="button"
                    disabled={c.state !== "idle"}
                    aria-label="停止"
                    title={c.state !== "idle" ? "空闲时才能停止" : "停止"}
                    className="flex size-5 items-center justify-center rounded hover:bg-selection disabled:opacity-40"
                    onClick={async () => {
                      const r = await command(c.conn, { type: "stop_task", task_id: t.id });
                      if (!r.success) setError(errorKey(c), `停止失败：${r.error?.message ?? ""}`);
                      void refresh(c.conn);
                    }}
                  >
                    <Square size={10} aria-hidden />
                  </button>
                )}
              </li>
            ))}
          </ul>
          {done > 0 && (
            <button type="button" onClick={() => setShowDone((v) => !v)} className="mt-1 px-2 text-[11px] text-secondary hover:text-text" data-testid="tasks-done">
              {showDone ? "隐藏已结束的" : `显示已结束的（${done}）`}
            </button>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
