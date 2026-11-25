import { describe, expect, it } from "vitest";
import type { AssistantBlock, Node } from "@/ipc/bindings";
import { asstNode, call, text, userNode } from "@/mocks/data/builders";
import { withCodeCalls } from "./codeCalls";
import type { CodeCall } from "./conn";

type CallBlock = Extract<AssistantBlock, { kind: "toolCall" }>;
const outcome = { isError: false, text: "r", log: "", truncated: false, durationMs: 5 };
const code = (id: string, callCount: number, done = false) =>
  call(id, "REPL", { code: "x" }, { text: "(dispatched #1)" }, { code: { callCount, outcome: done ? outcome : null } });
const made = (id: string, via: string) => ({
  ...(call(id, "Bash", { command: id }, { text: "" }) as CallBlock),
  viaCode: via,
});
const running = (id: string, codeId: string | null, name = "Bash"): CodeCall => ({
  toolUseId: id,
  name,
  input: { command: id },
  code: codeId,
  end: null,
});
const calls = (nodes: Node[], nodeId: string) => {
  const n = nodes.find((x) => x.id === nodeId)!;
  return n.body.kind === "assistant"
    ? n.body.blocks.flatMap((b) =>
        b.kind === "toolCall" ? [[b.toolUseId, b.viaCode, b.code?.callCount] as const] : [],
      )
    : [];
};

describe("withCodeCalls", () => {
  const nodes = [
    userNode("u1", 1, "go"),
    asstNode("a1", 2, [code("r1", 1), made("v1", "r1"), text("then"), code("r2", 0)]),
  ];

  it("puts a call after its code and the calls already listed for it, and counts it", () => {
    const out = withCodeCalls(nodes, [running("n1", "r1"), running("n2", "r2"), running("n3", "r1")]);
    expect(calls(out, "a1")).toEqual([
      ["r1", null, 3],
      ["v1", "r1", undefined],
      ["n1", "r1", undefined],
      ["n3", "r1", undefined],
      ["r2", null, 1],
      ["n2", "r2", undefined],
    ]);
    // The transcript itself is untouched, and so is a node that got nothing.
    expect(calls(nodes, "a1")).toHaveLength(3);
    expect(out[0]).toBe(nodes[0]);
  });

  it("a call the transcript has is not added again; with nothing to add the nodes are the same", () => {
    expect(withCodeCalls(nodes, [running("v1", "r1")])).toBe(nodes);
    expect(withCodeCalls(nodes, [])).toBe(nodes);
  });

  it("a call that names no code goes to the last code still running", () => {
    expect(calls(withCodeCalls(nodes, [running("n1", null)]), "a1").at(-1)).toEqual(["n1", "r2", undefined]);
  });

  it("a call whose code is not here, or already has its outcome, is left out", () => {
    const done = [userNode("u1", 1, "go"), asstNode("a1", 2, [code("r1", 0, true)])];
    expect(withCodeCalls(done, [running("n1", "r1"), running("n2", "gone"), running("n3", null)])).toBe(done);
  });

  it("an ended call carries the tool's answer; a question shows only once answered", () => {
    const ended: CodeCall = {
      ...running("n1", "r2"),
      end: { isError: true, response: { stdout: "out", stderr: "err" } },
    };
    const ask = running("q1", "r2", "AskUserQuestion");
    const out = withCodeCalls(nodes, [ended, ask]);
    expect(calls(out, "a1").map((c) => c[0])).toEqual(["r1", "v1", "r2", "n1"]);
    const a1 = out[1]!;
    const n1 = a1.body.kind === "assistant" ? (a1.body.blocks.at(-1) as CallBlock) : null;
    expect(n1?.result).toMatchObject({ isError: true, text: "out\nerr", extraJson: '{"stdout":"out","stderr":"err"}' });
    const answered = withCodeCalls(nodes, [{ ...ask, end: { isError: false, response: { answers: { q: "a" } } } }]);
    expect(calls(answered, "a1").at(-1)).toEqual(["q1", "r2", undefined]);
  });
});
