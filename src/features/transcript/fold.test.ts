import { describe, expect, it } from "vitest";
import type { Node } from "@/ipc/bindings";
import { asstNode, call, text, userNode } from "@/mocks/data/builders";
import { foldTurns } from "./fold";
import { buildRows, type GroupingInput, locate, rowIndexOf } from "./grouping";

const input = (nodes: Node[]): GroupingInput => ({
  nodes,
  branchPoints: [],
  subagents: [],
  workflows: [],
  inherited: null,
  orphanSubagentIds: [],
  showHidden: false,
  inheritedExpanded: false,
});
const ok = { text: "ok" };

const nodes = [
  userNode("u1", 1_000, "go"),
  asstNode("a1", 2_000, [
    text("先看看"),
    call("t1", "Read", { file_path: "/r/a.rs" }, ok),
    call("t2", "Bash", { command: "cargo test" }, ok),
  ]),
  asstNode("a2", 111_000, [text("改好了。")]),
  userNode("u2", 120_000, "谢谢"),
  asstNode("a3", 121_000, [text("不客气")]),
  userNode("u3", 130_000, "再跑一次"),
  asstNode("a4", 131_000, [text("在跑"), call("t3", "Bash", { command: "cargo test" }, null)]),
];

describe("按轮折叠", () => {
  const built = buildRows(input(nodes));

  it("a turn's process folds into one line with its time and counts; the answer and turns without work stay out", () => {
    const g = foldTurns(built, {});
    expect(g.rows.map((r) => r.kind)).toEqual([
      "prompt",
      "fold",
      "text",
      "prompt",
      "text",
      "prompt",
      "fold",
      "text",
      "tool",
    ]);
    expect(g.rows[1]).toMatchObject({ key: "fold:u1", open: false, ms: 110_000, texts: 1, calls: 2 });
    // Rows it hides point at it, so a jump lands on it.
    expect(rowIndexOf(g, "a1", "t1")).toBe(1);
    expect(rowIndexOf(g, "a2")).toBe(2);
  });

  it("the last turn without an answer (still running) starts open; a click decides after that", () => {
    const g = foldTurns(built, {});
    expect(g.rows[6]).toMatchObject({ key: "fold:u3", open: true });
    expect(
      foldTurns(built, { "fold:u3": false })
        .rows.map((r) => r.kind)
        .slice(5),
    ).toEqual(["prompt", "fold"]);
    expect(
      foldTurns(built, { "fold:u1": true })
        .rows.map((r) => r.kind)
        .slice(0, 6),
    ).toEqual(["prompt", "fold", "text", "tool", "tool", "text"]);
  });

  it("opened, a run of calls is not grouped: each call has its own row, and a jump to it lands there", () => {
    expect(built.rows.some((r) => r.kind === "group")).toBe(true);
    const g = foldTurns(built, { "fold:u1": true });
    expect(g.rows.some((r) => r.kind === "group")).toBe(false);
    expect(g.rows[rowIndexOf(g, "a1", "t1")!]).toMatchObject({ kind: "tool", call: { toolUseId: "t1" } });
    expect(g.rows[rowIndexOf(g, "a1", "t2")!]).toMatchObject({ kind: "tool", call: { toolUseId: "t2" } });
  });

  it("a jump into a folded process opens its fold", () => {
    expect(locate(foldTurns(built, {}), "a1", "t1")?.expand).toContain("fold:u1");
  });
});
