import { describe, expect, it } from "vitest";
import type { HaaTask } from "@/ipc/bindings";
import { asstNode, call, text, userNode } from "@/mocks/data/builders";
import { foldTurns } from "./fold";
import { buildRows, rowIndexOf } from "./grouping";
import { anchorNodeId, insertHaaTasks } from "./haaTasks";
import { input } from "./testHelpers";

const task = (endedMs: number, overrides: Partial<HaaTask> = {}): HaaTask => ({
  worker: "agy-w",
  sessionId: "s-w",
  title: `task-${endedMs}`,
  task: "do work",
  status: "done",
  model: "gemini-4-neon-high",
  endedMs,
  durationMs: 60_000,
  ...overrides,
});

describe("haaTasks placement", () => {
  const nodes = [
    userNode("u1", 1_000, "first"),
    asstNode("a1", 2_000, [text("working"), call("t1", "Bash", { command: "echo hi" }, { text: "hi" })]),
    asstNode("a2", 5_000, [text("done")]),
    userNode("h1", 6_000, "hidden meta", { hidden: true }),
    userNode("u2", 8_000, "second"),
    asstNode("a3", 9_000, [text("second answer")]),
  ];

  it("anchorNodeId picks the last non-hidden node with timestampMs <= endedMs, or null before first", () => {
    expect(anchorNodeId(nodes, 500)).toBeNull();
    expect(anchorNodeId(nodes, 1_000)).toBe("u1");
    expect(anchorNodeId(nodes, 2_500)).toBe("a1");
    // Skips hidden node h1 at 6_000.
    expect(anchorNodeId(nodes, 7_000)).toBe("a2");
    expect(anchorNodeId(nodes, 20_000)).toBe("a3");
  });

  it("places tasks at top level outside closed folds and shifts grouping.index", () => {
    const built = buildRows(input(nodes));
    const folded = foldTurns(built, {});
    // Sanity check: prompt(u1), fold(a1), text(a2), prompt(u2), text(a3)
    expect(folded.rows.map((r) => r.kind)).toEqual(["prompt", "fold", "text", "prompt", "text"]);

    const tBefore = task(500, { worker: "early" });
    const tInFold = task(3_000, { worker: "mid" });
    const tAfter = task(10_000, { worker: "late" });

    const withTasks = insertHaaTasks(folded, nodes, [tBefore, tInFold, tAfter]);
    expect(withTasks.rows.map((r) => r.kind)).toEqual([
      "haaTask",
      "prompt",
      "fold",
      "haaTask",
      "text",
      "prompt",
      "text",
      "haaTask",
    ]);
    // Cards are never marked with `fold`.
    for (const r of withTasks.rows) {
      if (r.kind === "haaTask") expect(r.fold).toBeUndefined();
    }
    // Index shifts stay accurate for scroll / jump targets.
    expect(rowIndexOf(withTasks, "u1")).toBe(1);
    expect(rowIndexOf(withTasks, "a1", "t1")).toBe(2);
    expect(rowIndexOf(withTasks, "a2")).toBe(4);
    expect(rowIndexOf(withTasks, "u2")).toBe(5);
    expect(rowIndexOf(withTasks, "a3")).toBe(6);
  });

  it("places task after the last row of a multi-row anchor node when fold is open", () => {
    const built = buildRows(input(nodes));
    const expanded = foldTurns(built, { "fold:u1": true });
    // prompt(u1), fold(open), text(a1), tool(a1), text(a2), prompt(u2), text(a3)
    expect(expanded.rows.map((r) => r.kind)).toEqual(["prompt", "fold", "text", "tool", "text", "prompt", "text"]);

    const withTasks = insertHaaTasks(expanded, nodes, [task(3_000, { worker: "mid" })]);
    expect(withTasks.rows.map((r) => r.kind)).toEqual([
      "prompt",
      "fold",
      "text",
      "tool",
      "haaTask",
      "text",
      "prompt",
      "text",
    ]);
    expect(rowIndexOf(withTasks, "a2")).toBe(5);
  });

  it("ignores hidden nodes even when showHidden is enabled and preserves order for same anchor", () => {
    const built = buildRows(input(nodes, { showHidden: true }));
    const folded = foldTurns(built, {});
    const t1 = task(6_500, { worker: "w1" });
    const t2 = task(7_000, { worker: "w2" });
    const withTasks = insertHaaTasks(folded, nodes, [t1, t2]);
    const a2Idx = rowIndexOf(withTasks, "a2")!;
    expect(withTasks.rows[a2Idx + 1]).toMatchObject({ kind: "haaTask", task: t1 });
    expect(withTasks.rows[a2Idx + 2]).toMatchObject({ kind: "haaTask", task: t2 });
  });

  it("returns the same grouping reference when tasks is empty", () => {
    const folded = foldTurns(buildRows(input(nodes)), {});
    expect(insertHaaTasks(folded, nodes, [])).toBe(folded);
  });
});
