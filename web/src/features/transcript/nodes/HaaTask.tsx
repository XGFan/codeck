import { Bot, Check, X } from "lucide-react";
import type { HaaTask } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatDuration } from "@/lib/format";
import { isReadById } from "@/features/haa/view";
import { useTranscriptCtx } from "./scroll";

/** The read-only iPhone Transcript page (`VITE_IPC=remote`) has no right panel, so open buttons stay hidden. */
const canOpenPanel = import.meta.env.VITE_IPC !== "remote";

function lastModelSegment(model: string | null | undefined): string {
  if (!model) return "";
  const i = model.lastIndexOf("/");
  return i >= 0 ? model.slice(i + 1) : model;
}

function TaskStatus({ status }: { status: string }) {
  if (status === "done") {
    return (
      <span
        data-testid="haa-task-status"
        data-state="done"
        className="flex shrink-0 items-center gap-1 text-[12px] text-live"
      >
        <Check size={12} strokeWidth={2.2} aria-hidden />
        已完成
      </span>
    );
  }
  if (status === "failed") {
    return (
      <span
        data-testid="haa-task-status"
        data-state="failed"
        className="flex shrink-0 items-center gap-1 text-[12px] text-error"
      >
        <X size={12} strokeWidth={2.2} aria-hidden />
        失败
      </span>
    );
  }
  return (
    <span
      data-testid="haa-task-status"
      data-state={status}
      className="flex shrink-0 items-center gap-1 text-[12px] text-secondary"
    >
      {status === "aborted" ? "已中止" : status}
    </span>
  );
}

/** Card for one completed haa Worker Task, styled after `SubagentCard`. */
export function HaaTaskCard({ task }: { task: HaaTask }) {
  const { activeWorkerId, openWorker } = useTranscriptCtx();
  const sessionId = isReadById(task.backend) ? task.sessionId : null;
  const active = Boolean(sessionId) && activeWorkerId === sessionId;
  const heading = task.title ?? task.task ?? "";
  const meta = [lastModelSegment(task.model), formatDuration(task.durationMs, "detail")].filter(Boolean).join(" · ");

  return (
    <div
      data-testid="haa-task-card"
      data-worker={task.worker ?? undefined}
      data-session-id={task.sessionId ?? undefined}
      data-active={active || undefined}
      className={cn(
        "flex w-full flex-col gap-1.5 rounded-[10px] border px-3 py-2.5",
        active
          ? "border-accent bg-selection/40 shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-accent)_12%,transparent)]"
          : "border-border bg-ground",
      )}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Bot size={14} strokeWidth={1.5} className="shrink-0 text-agy" aria-hidden />
        <span className="font-semibold">haa Worker</span>
        {task.worker && <span className="shrink-0 font-mono text-agy">{task.worker}</span>}
        <span className="min-w-[120px] flex-1 truncate">{heading}</span>
        {meta && <span className="text-[12px] text-secondary">{meta}</span>}
        {task.status && <TaskStatus status={task.status} />}
      </div>
      {task.task && (
        <div className="line-clamp-2 text-[12px] text-text/80">
          <span className="text-secondary">任务</span>
          <span className="ml-3">{task.task}</span>
        </div>
      )}
      {canOpenPanel && (
        <div className="flex items-center gap-3 text-[12px]">
          <span className="flex-1" />
          {sessionId ? (
            active ? (
              <span className="font-semibold text-accent">正在右侧查看</span>
            ) : (
              <button
                type="button"
                data-testid="haa-task-open"
                onClick={() => openWorker(sessionId)}
                className="font-semibold text-accent hover:underline"
              >
                查看完整过程 →
              </button>
            )
          ) : (
            <span className="text-secondary">对话未关联</span>
          )}
        </div>
      )}
    </div>
  );
}
