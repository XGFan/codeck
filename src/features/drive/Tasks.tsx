import { Layers, Square } from "lucide-react";
import { Popover } from "radix-ui";
import type { ConnState } from "./conn";
import { errorKey, useDrive } from "./store";

const KIND: Record<string, string> = { agent: "Subagent", shell: "Shell", workflow: "Workflow" };

/** Background tasks: a count beside the input box, the list (with stop) in a popover. */
export function Tasks({ c }: { c: ConnState }) {
  const command = useDrive((s) => s.command);
  const refresh = useDrive((s) => s.refreshTasks);
  const setError = useDrive((s) => s.setError);
  const running = c.tasks.filter((t) => t.status === "running");
  if (c.tasks.length === 0) return null;
  return (
    <Popover.Root onOpenChange={(o) => o && void refresh(c.conn)}>
      <Popover.Trigger className="flex h-6 items-center gap-1 rounded px-2 text-[12px] text-secondary hover:bg-selection" data-testid="tasks">
        <Layers size={12} aria-hidden /> {running.length > 0 ? `${running.length} 个后台任务` : "后台任务"}
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="end" sideOffset={4} className="z-50 w-96 rounded-md border border-border bg-ground p-2 shadow-lg">
          <ul className="max-h-80 space-y-1 overflow-y-auto">
            {c.tasks.map((t) => (
              <li key={t.id} className="flex items-center gap-2 rounded px-2 py-1 text-[12px]">
                <span className="shrink-0 text-secondary">{KIND[t.kind] ?? t.kind}</span>
                <span className="min-w-0 flex-1 truncate" title={t.command ?? t.description ?? ""}>
                  {t.description || t.command || t.id}
                </span>
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
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
