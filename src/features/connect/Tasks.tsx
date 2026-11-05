import { Network, Radar, Square, SquareTerminal, Workflow, type LucideIcon } from "lucide-react";
import { Popover } from "radix-ui";
import { footBtn } from "./Header";
import { Fragment, useMemo, useState } from "react";
import { useCurrentTranscript } from "@/queries";
import { useUi } from "@/state/ui";
import { scrollToNode } from "@/features/transcript/nodes/scroll";
import type { Transcript } from "@/ipc/bindings";
import type { ConnState } from "./conn";
import type { Task } from "./protocol";
import { errorKey, useConnect } from "./store";

/** The kinds in the order they are listed; an unknown kind (a newer ccrpc) goes last under its own name. */
const KINDS: { kind: string; label: string; Icon: LucideIcon }[] = [
  { kind: "shell", label: "Shell", Icon: SquareTerminal },
  { kind: "agent", label: "Subagent", Icon: Network },
  { kind: "monitor", label: "Monitor", Icon: Radar },
  { kind: "workflow", label: "Workflow", Icon: Workflow },
];
const kindOf = (kind: string) => KINDS.find((k) => k.kind === kind) ?? { kind, label: kind, Icon: SquareTerminal };

const STATUS: Record<string, string> = {
  running: "运行中",
  completed: "已完成",
  failed: "失败",
  error: "失败",
  killed: "已终止",
  stopped: "已停止",
};

/** Tasks grouped by kind, in `KINDS` order. */
function byKind(tasks: Task[]): [string, Task[]][] {
  const groups = new Map<string, Task[]>();
  for (const k of KINDS) groups.set(k.kind, []);
  for (const t of tasks) groups.set(t.kind, [...(groups.get(t.kind) ?? []), t]);
  return [...groups].filter(([, ts]) => ts.length > 0);
}

/** toolUseId → id of the main-line node that made the call. */
function toolNodes(transcript: Transcript | undefined): Map<string, string> {
  const m = new Map<string, string>();
  for (const n of transcript?.nodes ?? [])
    if (n.body.kind === "assistant") for (const b of n.body.blocks) if (b.kind === "toolCall") m.set(b.toolUseId, n.id);
  return m;
}

/**
 * Background tasks: beside the input box, the running ones counted by kind (Shell / Subagent / Monitor / Workflow;
 * nothing when none runs); the list (grouped by kind, with stop) in a popover, finished ones only on request. A
 * Subagent opens its Subagent Run in the side panel; the others jump to the tool call that started them.
 */
export function Tasks({ c }: { c: ConnState }) {
  const command = useConnect((s) => s.command);
  const refresh = useConnect((s) => s.refreshTasks);
  const setError = useConnect((s) => s.setError);
  const [showDone, setShowDone] = useState(false);
  const openSession = useUi((s) => s.sessionId);
  const setPanelStack = useUi((s) => s.setPanelStack);
  const { data: transcript } = useCurrentTranscript();
  const shown = c.sessionId === openSession ? transcript : undefined;
  const runs = useMemo(() => new Map(shown?.subagents.map((r) => [r.agentId, r])), [shown]);
  const calls = useMemo(() => toolNodes(shown), [shown]);
  // A nested run opens under its parents, as its card's 「进入」 would.
  const stackOf = (agentId: string) => {
    const stack: string[] = [];
    for (let id: string | null | undefined = agentId; id && !stack.includes(id); id = runs.get(id)?.parentAgentId)
      stack.unshift(id);
    return stack;
  };
  const running = c.tasks.filter((t) => t.status === "running");
  const done = c.tasks.length - running.length;
  if (running.length === 0) return null;
  const counts = byKind(running).map(([kind, ts]) => ({ ...kindOf(kind), n: ts.length }));
  const groups = byKind(showDone ? c.tasks : running);

  const name = (t: Task) => t.description || t.command || t.id;
  const open = (t: Task) => {
    if (t.kind === "agent")
      return runs.has(t.id) ? { title: "在右侧查看完整过程", go: () => setPanelStack(stackOf(t.id)) } : null;
    const nodeId = t.tool_use_id ? calls.get(t.tool_use_id) : undefined;
    if (!nodeId) return null;
    return {
      title: "跳到启动它的工具调用",
      go: () => scrollToNode({ scope: { kind: "main" }, nodeId, toolUseId: t.tool_use_id }),
    };
  };

  return (
    <Popover.Root onOpenChange={(o) => o && void refresh(c.conn)}>
      <Popover.Trigger
        className={footBtn}
        data-testid="tasks"
        title={`后台运行中：${counts.map((k) => `${k.n} 个 ${k.label}`).join("、")}`}
      >
        {counts.map((k, i) => (
          <Fragment key={k.kind}>
            {i > 0 && (
              <span className="opacity-40" aria-hidden>
                ·
              </span>
            )}
            <span data-testid={`tasks-${k.kind}`} className="flex items-center gap-1">
              <k.Icon size={14} strokeWidth={1.7} aria-hidden />
              {k.n} {k.label}
            </span>
          </Fragment>
        ))}
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={4}
          className="z-50 w-96 rounded-md border border-border bg-ground p-2 shadow-lg"
        >
          <div className="max-h-80 space-y-2 overflow-y-auto" data-testid="tasks-list">
            {groups.map(([kind, ts]) => {
              const k = kindOf(kind);
              return (
                <section key={kind} data-testid="tasks-group" data-kind={kind}>
                  <h3 className="flex items-center gap-1.5 px-2 pb-0.5 text-[11px] font-medium text-secondary">
                    <k.Icon size={12} strokeWidth={1.8} aria-hidden />
                    {k.label}
                  </h3>
                  <ul className="space-y-0.5">
                    {ts.map((t) => {
                      const target = open(t);
                      const sub =
                        (t.kind === "shell" || t.kind === "monitor") && t.command !== name(t) ? t.command : null;
                      return (
                        <li key={t.id} className="flex items-center gap-2 rounded px-2 py-1 text-[12px]">
                          <div className="min-w-0 flex-1">
                            {target ? (
                              <Popover.Close
                                data-testid={t.kind === "agent" ? "task-open" : "task-jump"}
                                onClick={target.go}
                                title={target.title}
                                className="block max-w-full truncate text-left text-accent hover:underline"
                              >
                                {name(t)}
                              </Popover.Close>
                            ) : (
                              <span className="block truncate" title={t.command ?? t.description ?? ""}>
                                {name(t)}
                              </span>
                            )}
                            {sub && (
                              <span className="block truncate font-mono text-[11px] text-secondary" title={sub}>
                                {sub}
                              </span>
                            )}
                          </div>
                          <span className="shrink-0 text-secondary">{STATUS[t.status] ?? t.status}</span>
                          {t.status === "running" && (
                            <button
                              type="button"
                              disabled={c.state !== "idle"}
                              aria-label="停止"
                              title={c.state !== "idle" ? "空闲时才能停止" : "停止"}
                              className="flex size-5 shrink-0 items-center justify-center rounded hover:bg-selection disabled:opacity-40"
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
                      );
                    })}
                  </ul>
                </section>
              );
            })}
          </div>
          {done > 0 && (
            <button
              type="button"
              onClick={() => setShowDone((v) => !v)}
              className="mt-1 px-2 text-[11px] text-secondary hover:text-text"
              data-testid="tasks-done"
            >
              {showDone ? "隐藏已结束的" : `显示已结束的（${done}）`}
            </button>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
