import { describe, expect, it, vi } from "vitest";
import { asstNode, otherNode, text, userNode } from "@/mocks/data/builders";
import { addWithdrawn, hideNodes, loadWithdrawn, withdrawnPrompt } from "./withdrawn";

const att = (id: string) =>
  otherNode(id, 3, { kind: "attachment", attachmentType: "skill_listing", text: "" }, { hidden: true });

describe("withdrawnPrompt", () => {
  it("finds the last prompt with no reply after it that reads the restored text", () => {
    const nodes = [
      userNode("u1", 1, "先看看"),
      asstNode("a1", 2, [text("好")]),
      userNode("u2", 3, "改成\n两行"),
      att("t"),
    ];
    expect(withdrawnPrompt(nodes, "改成 两行")).toBe("u2");
  });

  it("not a prompt with a reply after it, one with other text, or a command", () => {
    const replied = [userNode("u1", 1, "改一下"), asstNode("a1", 2, [text("好")])];
    expect(withdrawnPrompt(replied, "改一下")).toBeNull();
    expect(withdrawnPrompt([userNode("u1", 1, "别的")], "改一下")).toBeNull();
    const cmd = userNode("c1", 1, "/model", { origin: { kind: "command", name: "/model", args: "" } });
    expect(withdrawnPrompt([cmd], "/model")).toBeNull();
  });

  it("passes over a prompt taken back already: the same words sent again and taken back again", () => {
    const nodes = hideNodes([userNode("u1", 1, "再看看"), userNode("u2", 2, "再看看")], ["u1"]);
    expect(withdrawnPrompt(nodes, "再看看")).toBe("u2");
  });

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
