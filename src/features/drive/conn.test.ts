import { describe, expect, it } from "vitest";
import { apply, initial, type ConnState } from "./conn";
import type { InMsg, PermissionRequestEvent, TurnEndEvent } from "./protocol";

const fold = (msgs: InMsg[], s: ConnState = initial("c1", "h1")) => msgs.reduce(apply, s);
const perm = (id: string, agent: string | null = null): PermissionRequestEvent => ({
  type: "permission_request",
  request_id: id,
  tool_name: "Bash",
  input: { command: "ls" },
  agent_id: agent,
  options: null,
  layout_source: null,
});
const end: TurnEndEvent = {
  type: "turn_end",
  prompt_id: "p",
  trigger: "prompt",
  last_text: "ok",
  error: null,
  aborted: false,
  background_tasks: [],
  duration_ms: 10,
  usage: { input_tokens: 1, output_tokens: 1, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
  api_cost: 0,
};

describe("conn reducer", () => {
  it("ends main-line requests when waiting ends, a background Subagent's when its tool ran", () => {
    const waiting: InMsg = { type: "state", state: "waiting", waiting: { kind: "permission", waiting_for: "permission prompt" } };
    let s = fold([waiting, perm("t1"), perm("t2", "agent-1")]);
    expect(s.requests.map((r) => r.request_id)).toEqual(["t1", "t2"]);
    s = apply(s, { type: "state", state: "busy" });
    expect(s.requests.map((r) => r.request_id)).toEqual(["t2"]);
    // Text blocks change nothing; the Subagent's own tool result ends its request.
    expect(apply(s, { type: "block", kind: "text" })).toBe(s);
    s = apply(s, { type: "block", kind: "tool_result", tool_use_id: "t2" });
    expect(s.requests).toEqual([]);
  });

  it("re-announcing a request replaces it; a new screen card replaces the previous one", () => {
    let s = fold([perm("t1"), { ...perm("t1"), options: [{ label: "Yes", meaning: "allow" }] }]);
    expect(s.requests).toHaveLength(1);
    expect((s.requests[0] as PermissionRequestEvent).options).toHaveLength(1);
    s = fold([{ type: "dialog", waiting_for: "x", lines: ["a"] }, { type: "unrecognized", context: "c", lines: ["b"] }], s);
    expect(s.requests.map((r) => r.type)).toEqual(["permission_request", "unrecognized"]);
  });

  it("keeps the last turn's end and shows progress only while busy", () => {
    let s = fold([{ type: "progress", text: "Thinking…", verb: "Thinking", elapsed_s: 1, tokens: null }]);
    expect(s.progress).toBeNull();
    s = fold([{ type: "state", state: "busy" }, { type: "progress", text: "Thinking…", verb: "Thinking", elapsed_s: 1, tokens: null }], s);
    expect(s.progress?.text).toBe("Thinking…");
    s = apply(s, end);
    expect(s.progress).toBeNull();
    expect(s.lastTurn).toBe(end);
  });

  it("starts over on session_changed, keeping what describes the Pane", () => {
    const ready: InMsg = {
      type: "ready",
      protocol: 2,
      holder_id: "h1",
      session_id: "old",
      cwd: "/w",
      claude_version: "2.1",
      state: "idle",
      size: [80, 24],
      taken_over: false,
      co_display: true,
      mode: "manual",
      model: "m",
      context_tokens: 5,
      warnings: [],
    };
    let s = fold([ready, perm("t1"), end]);
    s = apply(s, { type: "session_changed", session_id: "new", previous: "old" });
    expect(s.sessionId).toBe("new");
    expect(s.ready).toBe(true);
    expect(s.cwd).toBe("/w");
    expect(s.mode).toBe("manual");
    expect(s.requests).toEqual([]);
    expect(s.lastTurn).toBeNull();
  });
});
