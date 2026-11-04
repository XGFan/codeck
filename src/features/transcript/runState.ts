import type { SubagentRun, ToolCall } from "@/ipc/bindings";

export const FAILED = new Set(["failed", "error", "killed", "cancelled"]);
export const DONE = new Set(["completed", "success", "succeeded", "done"]);

/** `noNotice`: a background run whose Session is no longer live and whose end notification never came. */
export type RunState = "running" | "done" | "failed" | "noNotice";

/**
 * Whether a Subagent Run has ended. A background run ends with its task-notification (until then its status is
 * `async_launched`); a foreground one with its Agent tool_result. A run that has not ended is running while its
 * Session is live; `null` when nothing tells: an unlinked run, a Teammate (its result only says it was spawned, and
 * nothing records its end), a foreground run without a result in an ended Session.
 */
export function runState(run: SubagentRun, call: ToolCall | undefined, live: boolean): RunState | null {
  if (FAILED.has(run.status ?? "") || call?.result?.isError === true) return "failed";
  if (run.isAsync) {
    if (DONE.has(run.status ?? "")) return "done";
    return live ? "running" : "noNotice";
  }
  if (run.status === "teammate_spawned") return null;
  if (call?.result || (!call && DONE.has(run.status ?? ""))) return "done";
  if (!run.toolUseId) return null;
  return live ? "running" : null;
}
