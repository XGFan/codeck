import { describe, expect, it } from "vitest";
import type { Block, InMsg, PermissionRequestEvent } from "@/protocol";
import { apply, applyPage, initial, type ConnState } from "./conn";

const text = (uuid: string, t = uuid, extra: Partial<Block> = {}): Block =>
  ({ type: "block", kind: "text", uuid, index: 0, text: t, ...extra }) as Block;
const fold = (msgs: InMsg[], s: ConnState = initial("c1", "h1")) => msgs.reduce(apply, s);
const uuids = (s: ConnState) => s.items.flatMap((i) => (i.kind === "block" ? [i.block.uuid] : []));
const perm = (id: string, agent: string | null = null): PermissionRequestEvent => ({
  type: "permission_request",
  request_id: id,
  tool_name: "Bash",
  input: { command: "ls" },
  agent_id: agent,
  options: null,
  layout_source: null,
});

describe("conn reducer", () => {
  it("drops blocks a branch invalidated, keeping every block of the last valid entry", () => {
    const s = fold([
      text("a"),
      { ...text("b"), index: 0 } as Block,
      { ...text("b"), index: 1 } as Block,
      text("c"),
      { type: "branch", at_uuid: "b" },
      text("d"),
    ]);
    expect(s.items.map((i) => i.key)).toEqual(["a:0", "b:0", "b:1", "d:0"]);
    // A block cut by the branch may come again on the new Branch.
    expect(uuids(apply(s, text("c")))).toEqual(["a", "b", "b", "d", "c"]);
    expect(uuids(apply(s, { type: "branch", at_uuid: null }))).toEqual([]);
  });

  it("starts over (to reload) on a branch before the loaded blocks", () => {
    const s = applyPage(fold([]), { blocks: [text("x"), text("y")], cursor: 5, first: 3, more: true }, false);
    const b = apply(s, { type: "branch", at_uuid: "older-than-the-page" });
    expect(b.items).toEqual([]);
    expect(b.loaded).toBe(false);
    expect(b.first).toBeNull();
  });

  it("puts the first page before live blocks that arrived meanwhile, without duplicates", () => {
    const live = fold([text("c"), text("d")]);
    const s = applyPage(live, { blocks: [text("a"), text("b"), text("c")], cursor: 9, first: 3, more: true }, false);
    expect(uuids(s)).toEqual(["a", "b", "c", "d"]);
    expect(s.first).toBe(3);
    expect(s.more).toBe(true);
    const older = applyPage(s, { blocks: [text("z")], cursor: 3, first: 0, more: false }, true);
    expect(uuids(older)).toEqual(["z", "a", "b", "c", "d"]);
    expect(older.more).toBe(false);
  });

  it("routes Subagent and Workflow blocks under their parent call, not the Main Line", () => {
    const s = fold([
      text("m"),
      text("s1", "x", { agent_id: "ag", parent_tool_use_id: "toolu_A" }),
      text("s1", "x", { agent_id: "ag", parent_tool_use_id: "toolu_A" }),
      text("w1", "y", { agent_id: "w-ag", run_id: "r1", parent_tool_use_id: "toolu_W" }),
    ]);
    expect(uuids(s)).toEqual(["m"]);
    expect(s.sub.toolu_A.map((b) => b.uuid)).toEqual(["s1"]);
    expect(s.sub.toolu_W.map((b) => b.uuid)).toEqual(["w1"]);
  });

  it("ends main-line requests when waiting ends, a background Subagent's when its tool ran", () => {
    const waiting: InMsg = { type: "state", state: "waiting", waiting: { kind: "permission", waiting_for: "permission prompt" } };
    let s = fold([waiting, perm("t1"), perm("t2", "agent-1")]);
    expect(s.requests.map((r) => r.request_id)).toEqual(["t1", "t2"]);
    s = apply(s, { type: "state", state: "busy" });
    expect(s.requests.map((r) => r.request_id)).toEqual(["t2"]);
    // The Subagent's own tool result (it carries agent_id) ends it.
    s = apply(s, {
      type: "block",
      kind: "tool_result",
      uuid: "r",
      index: 0,
      tool_use_id: "t2",
      content: "",
      is_error: false,
      agent_id: "agent-1",
      parent_tool_use_id: "toolu_A",
    });
    expect(s.requests).toEqual([]);
    expect(s.sub.toolu_A).toHaveLength(1);
  });

  it("re-announcing a request replaces it; a new screen card replaces the previous one", () => {
    let s = fold([perm("t1"), { ...perm("t1"), options: [{ label: "Yes", meaning: "allow" }] }]);
    expect(s.requests).toHaveLength(1);
    expect((s.requests[0] as PermissionRequestEvent).options).toHaveLength(1);
    s = fold([{ type: "dialog", waiting_for: "x", lines: ["a"] }, { type: "unrecognized", context: "c", lines: ["b"] }], s);
    expect(s.requests.map((r) => r.type)).toEqual(["permission_request", "unrecognized"]);
  });

  it("starts over on session_changed and keeps the turn footer in the stream", () => {
    const end: InMsg = {
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
    let s = fold([text("a"), end]);
    expect(s.items.map((i) => i.kind)).toEqual(["block", "turn_end"]);
    s = apply(s, { type: "session_changed", session_id: "new", previous: "old" });
    expect(s.items).toEqual([]);
    expect(s.sessionId).toBe("new");
    expect(s.ready).toBe(true);
  });
});
