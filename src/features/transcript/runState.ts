import type { SubagentRun, ToolCall } from "@/ipc/bindings";

export const FAILED = new Set(["failed", "error", "killed", "cancelled"]);
export const DONE = new Set(["completed", "success", "succeeded", "done"]);

/**
 * `noNotice`: a background run whose Session is no longer live and whose end notification never came. `idle` /
 * `exited`: a Teammate between rounds / shut down.
 */
export type RunState = "running" | "done" | "failed" | "noNotice" | "idle" | "exited";

/**
 * Whether a Subagent Run has ended. A background run ends with its task-notification (until then its status is
 * `async_launched`); a foreground one with its Agent tool_result. A run that has not ended is running while its
 * Session is live; `null` when nothing tells: an unlinked run, a foreground run without a result in an ended Session.
 * A Teammate's status comes from what it relays to the lead: `exited` once it shut down, else running / idle while the
 * Session is live (after that, nothing tells).
 */
export function runState(run: SubagentRun, call: ToolCall | undefined, live: boolean): RunState | null {
  if (FAILED.has(run.status ?? "") || call?.result?.isError === true) return "failed";
  if (run.teammate) {
    if (run.status === "exited") return "exited";
    if (!live) return null;
    return run.status === "idle" ? "idle" : "running";
  }
  if (run.isAsync) {
    if (DONE.has(run.status ?? "")) return "done";
    return live ? "running" : "noNotice";
  }
  if (call?.result || (!call && DONE.has(run.status ?? ""))) return "done";
  if (!run.toolUseId) return null;
  return live ? "running" : null;
}
