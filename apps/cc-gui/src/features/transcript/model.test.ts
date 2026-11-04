import { describe, expect, it } from "vitest";
import type { Block, TurnEndEvent } from "@/protocol";
import type { Item } from "@/state/conn";
import { buildRows, buildTurns, findCall, TRUNCATED_MARK } from "./model";

let seq = 0;
const base = () => ({ type: "block" as const, uuid: `u${++seq}`, index: 0 });
const user = (text: string, queued = false): Block => ({ ...base(), kind: "user", text, ...(queued ? { queued } : {}) });
const text = (t: string): Block => ({ ...base(), kind: "text", text: t });
const thinking = (t: string): Block => ({ ...base(), kind: "thinking", text: t });
const use = (id: string, name: string, input: unknown = {}): Block => ({ ...base(), kind: "tool_use", id, name, input });
const result = (id: string, content = "ok", is_error = false): Block => ({ ...base(), kind: "tool_result", tool_use_id: id, content, is_error });

const end: TurnEndEvent = {
  type: "turn_end",
  prompt_id: null,
  trigger: "prompt",
  last_text: null,
  error: null,
  aborted: false,
  background_tasks: [],
  duration_ms: 1000,
  usage: { input_tokens: 1, output_tokens: 2, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
  api_cost: 0.01,
};
const items = (...xs: (Block | TurnEndEvent)[]): Item[] =>
  xs.map((x, i) => (x.type === "turn_end" ? { kind: "turn_end", key: `turn:${i}`, ev: x } : { kind: "block", key: `${x.uuid}:${x.index}`, block: x }));

describe("buildRows", () => {
  it("shows a background task's end as a note, not the user's bubble", () => {
    const n = user(
      '<task-notification>\n<task-id>a69</task-id>\n<status>completed</status>\n<summary>Agent "Review fix" finished</summary>\n<note>A task-notification fires…</note>\n</task-notification>',
    );
    const rows = buildRows([n, text("看了一下结果")]);
    expect(rows[0]).toMatchObject({ kind: "task_note", status: "completed", summary: 'Agent "Review fix" finished' });
    expect(rows[1]).toMatchObject({ kind: "text" });
    expect(buildRows([user("我说的 <task-notification> 是什么")])[0]).toMatchObject({ kind: "user" });
  });

  it("pairs each tool_use with its tool_result and drops the result row", () => {
    const rows = buildRows([use("a", "Bash", { command: "ls" }), result("a", "file", false)]);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ kind: "tool", call: { id: "a", name: "Bash", input: { command: "ls" }, result: { content: "file", isError: false } } });
  });

  it("groups consecutive calls, keeps thinking between them inside, and counts failures", () => {
    const rows = buildRows([
      use("a", "Read"),
      thinking("hmm"),
      use("b", "Read"),
      use("c", "Bash"),
      result("a"),
      result("b", "boom", true),
      result("c"),
      text("done"),
    ]);
    expect(rows.map((r) => r.kind)).toEqual(["group", "text"]);
    const g = rows[0] as Extract<(typeof rows)[number], { kind: "group" }>;
    expect(g.callCount).toBe(3);
    expect(g.failedCount).toBe(1);
    expect(g.counts).toEqual([
      ["Read", 2],
      ["Bash", 1],
    ]);
    expect(g.items.map((i) => i.type)).toEqual(["call", "thinking", "call", "call"]);
  });

  it("emits held thinking after a lone call, and Agent / Workflow calls as cards that break groups", () => {
    const rows = buildRows([use("a", "Grep"), thinking("t"), use("b", "Agent", { description: "x" }), use("c", "Workflow"), use("d", "Read")]);
    expect(rows.map((r) => r.kind)).toEqual(["tool", "thinking", "agent", "agent", "tool"]);
  });

  it("marks truncated and persisted results, and keeps a result without its call", () => {
    const rows = buildRows([use("a", "Bash"), result("a", `x${TRUNCATED_MARK}`), result("zz", "orphan")]);
    expect(rows[0]).toMatchObject({ kind: "tool", call: { result: { truncated: true } } });
    expect(rows[1]).toMatchObject({ kind: "result", toolUseId: "zz", result: { content: "orphan" } });
  });

  it("marks calls without a result unfinished only when told the run is over", () => {
    expect(buildRows([use("a", "Bash")])[0]).toMatchObject({ call: { unfinished: false } });
    expect(buildRows([use("a", "Bash")], undefined, true)[0]).toMatchObject({ call: { unfinished: true } });
  });
});

describe("buildTurns", () => {
  it("starts a turn at each prompt, keeps a queued steer inside the turn, and closes it with turn_end", () => {
    const turns = buildTurns(items(user("hi"), text("a"), user("steer", true), text("b"), end, user("next"), use("x", "Bash")));
    expect(turns).toHaveLength(2);
    expect(turns[0]!.rows.map((r) => r.kind)).toEqual(["user", "text", "user", "text", "turn_end"]);
    expect(turns[0]!.rows[2]).toMatchObject({ kind: "user", queued: true });
    // The last turn is still running: its call is not unfinished.
    expect(turns[1]!.rows[1]).toMatchObject({ kind: "tool", call: { unfinished: false } });
  });

  it("treats calls of an earlier turn without a result as unfinished, and content after turn_end as a new turn", () => {
    const turns = buildTurns(items(user("a"), use("x", "Bash"), user("b"), text("r"), end, text("notified")));
    expect(turns).toHaveLength(3);
    expect(turns[0]!.rows[1]).toMatchObject({ call: { unfinished: true } });
    expect(turns[2]!.rows.map((r) => r.kind)).toEqual(["text"]);
  });

  it("pairs a result that arrives in a later item of the same turn", () => {
    const turns = buildTurns(items(user("a"), use("x", "Read"), text("between"), result("x", "content")));
    expect(turns[0]!.rows[1]).toMatchObject({ kind: "tool", call: { result: { content: "content" } } });
  });
});

describe("findCall", () => {
  it("finds a call on the Main Line or nested in a Subagent Run", () => {
    const main = items(use("agent1", "Agent", { description: "d" }), result("agent1", "done"));
    const sub = { agent1: [use("nested", "Agent"), result("nested", "inner")] };
    expect(findCall(main, sub, "agent1")).toMatchObject({ name: "Agent", input: { description: "d" }, result: { content: "done" } });
    expect(findCall(main, sub, "nested")).toMatchObject({ result: { content: "inner" } });
    expect(findCall(main, sub, "nope")).toBeNull();
  });
});

describe("interruptions", () => {
  it("stay in the turn they interrupted", () => {
    const item = (b: Record<string, unknown>) => ({ kind: "block" as const, key: `${b.uuid}:0`, block: { type: "block", index: 0, ...b } as never });
    const turns = buildTurns([
      item({ kind: "user", uuid: "u1", text: "do it" }),
      item({ kind: "text", uuid: "a1", text: "working" }),
      item({ kind: "user", uuid: "u2", text: "[Request interrupted by user]" }),
      item({ kind: "user", uuid: "u3", text: "next" }),
    ]);
    expect(turns.map((t) => t.rows.map((r) => r.kind))).toEqual([["user", "text", "user"], ["user"]]);
  });
});
