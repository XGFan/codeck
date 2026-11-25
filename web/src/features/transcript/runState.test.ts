import { describe, expect, it } from "vitest";
import type { SubagentRun, ToolCall } from "@/ipc/bindings";
import { runState } from "./runState";

const run = (over: Partial<SubagentRun>): SubagentRun =>
  ({ toolUseId: "tu1", isAsync: false, status: null, ...over }) as SubagentRun;
const call = (result: ToolCall["result"]): ToolCall => ({ toolUseId: "tu1", result }) as ToolCall;
const ok = { text: "done", isError: false } as ToolCall["result"];

describe("runState", () => {
  it("a background run is running until its notification, however its launch result reads", () => {
    const launched = run({ isAsync: true, status: "async_launched" });
    expect(runState(launched, call(ok), true)).toBe("running");
    expect(runState(launched, call(ok), false)).toBe("noNotice");
    expect(runState(run({ isAsync: true, status: "completed" }), call(ok), false)).toBe("done");
    expect(runState(run({ isAsync: true, status: "failed" }), call(ok), true)).toBe("failed");
  });

  it("a foreground run ends with its tool_result", () => {
    expect(runState(run({}), call(null), true)).toBe("running");
    expect(runState(run({}), call(null), false)).toBeNull();
    expect(runState(run({ status: "completed" }), call(ok), true)).toBe("done");
    expect(runState(run({}), call({ ...ok!, isError: true }), false)).toBe("failed");
    // In the panel there is no call: the status says it.
    expect(runState(run({ status: "completed" }), undefined, true)).toBe("done");
  });

  it("a Teammate runs, goes idle and exits by what it relays; its spawn result is not its end", () => {
    const mate = (status: string) => run({ teammate: true, status });
    expect(runState(mate("running"), call(ok), true)).toBe("running");
    expect(runState(mate("idle"), call(ok), true)).toBe("idle");
    expect(runState(mate("exited"), call(ok), false)).toBe("exited");
    // The Session ended without its shutdown: nothing tells.
    expect(runState(mate("idle"), call(ok), false)).toBeNull();
  });

  it("an unlinked run says nothing", () => {
    expect(runState(run({ toolUseId: null }), undefined, true)).toBeNull();
  });
});
