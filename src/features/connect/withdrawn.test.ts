import { describe, expect, it, vi } from "vitest";
import { userNode } from "@/mocks/data/builders";
import { addWithdrawn, hideNodes, loadWithdrawn } from "./withdrawn";

describe("hideNodes", () => {
  it("hides the nodes by id", () => {
    const nodes = [userNode("u1", 1, "a"), userNode("u2", 2, "b")];
    expect(hideNodes(nodes, ["u2"]).map((n) => n.hidden)).toEqual([false, true]);
    expect(hideNodes(nodes, [])).toBe(nodes);
  });
});

describe("remembered per Session", () => {
  it("adds an id once, the Session moving to the end, and keeps the newest 200 Sessions", () => {
    const a = addWithdrawn({ s1: ["u1"], s2: ["u9"] }, "s1", "u2");
    expect(Object.entries(a)).toEqual([
      ["s2", ["u9"]],
      ["s1", ["u1", "u2"]],
    ]);
    expect(addWithdrawn(a, "s1", "u2")).toBe(a);
    let many: Record<string, string[]> = {};
    for (let i = 0; i < 205; i++) many = addWithdrawn(many, `s${i}`, "u");
    expect(Object.keys(many)).toHaveLength(200);
    expect(many.s0).toBeUndefined();
    expect(many.s204).toEqual(["u"]);
  });

  it("reads nothing from garbled storage", () => {
    const storage = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => storage.get(k) ?? null,
      setItem: (k: string, v: string) => void storage.set(k, v),
    });
    localStorage.setItem("codeck.withdrawn", "{not json");
    expect(loadWithdrawn()).toEqual({});
    localStorage.setItem("codeck.withdrawn", JSON.stringify({ s1: ["u1"], s2: "x", s3: [1] }));
    expect(loadWithdrawn()).toEqual({ s1: ["u1"] });
    vi.unstubAllGlobals();
  });
});
