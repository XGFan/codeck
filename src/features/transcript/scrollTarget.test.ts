import { describe, expect, it } from "vitest";
import type { Node } from "@/ipc/bindings";
import { asstNode, call, userNode } from "@/mocks/data/builders";
import { buildRows, type GroupingInput } from "./grouping";
import { resolveScrollTarget, type ScrollMiss } from "./scrollTarget";

const full = (nodes: Node[]) =>
  buildRows({
    nodes,
    branchPoints: [],
    subagents: [],
    workflows: [],
    inherited: null,
    orphanSubagentIds: [],
    showHidden: false,
    inheritedExpanded: true,
  } satisfies GroupingInput);

const nodes = [
  userNode("u1", 1, "go"),
  asstNode("a1", 2, [call("t1", "Bash", {}, { text: "ok" }), call("t2", "Bash", {}, { text: "ok" })]),
];
const req = (nodeId: string, toolUseId?: string) => ({ scope: { kind: "main" } as const, nodeId, toolUseId, seq: 7 });

describe("resolveScrollTarget", () => {
  it("found: the expansion keys that reveal the call", () => {
    expect(resolveScrollTarget(full(nodes), req("a1", "t2"), 1, null)).toEqual({
      kind: "found",
      expand: ["fold:u1", "group:a1|t1", "tool:a1|t2"],
    });
  });

  it("missing: the first miss starts the bound; the same revision keeps waiting", () => {
    const first = resolveScrollTarget(full(nodes), req("a9"), 1, null);
    expect(first).toEqual({ kind: "wait", miss: { seq: 7, revision: 1, changes: 0 }, first: true });
    const again = resolveScrollTarget(full(nodes), req("a9"), 1, (first as { miss: ScrollMiss }).miss);
    expect(again).toEqual({ kind: "wait", miss: { seq: 7, revision: 1, changes: 0 }, first: false });
  });

  it("missing: dropped after two data updates; a new request starts over", () => {
    const miss0: ScrollMiss = { seq: 7, revision: 1, changes: 0 };
    const r2 = resolveScrollTarget(full(nodes), req("a9"), 2, miss0);
    expect(r2).toEqual({ kind: "wait", miss: { seq: 7, revision: 2, changes: 1 }, first: false });
    const r3 = resolveScrollTarget(full(nodes), req("a9"), 3, (r2 as { miss: ScrollMiss }).miss);
    expect(r3).toEqual({ kind: "drop", miss: { seq: 7, revision: 3, changes: 2 } });
    expect(resolveScrollTarget(full(nodes), { ...req("a9"), seq: 8 }, 3, miss0)).toMatchObject({ first: true });
  });

  it("a node that arrives later is found", () => {
    const more = [...nodes, asstNode("a9", 3, [call("t9", "Read", {}, { text: "ok" })])];
    expect(resolveScrollTarget(full(more), req("a9"), 2, { seq: 7, revision: 1, changes: 0 })).toMatchObject({
      kind: "found",
    });
  });
});
