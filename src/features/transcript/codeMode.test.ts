import { describe, expect, it } from "vitest";
import { asstNode, call, userNode } from "@/mocks/data/builders";
import { buildRows, describeCalls, locate, rowIndexOf, type Row } from "./grouping";
import { groups, input } from "./testHelpers";

describe("Code Mode", () => {
  type CallBlock = Extract<ReturnType<typeof call>, { kind: "toolCall" }>;
  const inner = (id: string, name: string, input: unknown, isError = false) =>
    call(id, name, input, { text: "", isError }) as CallBlock;
  /** Claude's code and, right after it, the calls it made (as the backend lists them). */
  const repl = (id: string, calls: CallBlock[], threw = false) => [
    call(
      id,
      "REPL",
      { code: "x" },
      { text: "(dispatched #1)" },
      {
        code: {
          callCount: calls.length,
          outcome: { isError: threw, text: "r", log: "", truncated: false, durationMs: 5 },
        },
      },
    ),
    ...calls.map((c) => ({ ...c, viaCode: id })),
  ];
  const callIds = (r: Row) =>
    r.kind === "group" ? r.items.flatMap((it) => (it.type === "call" ? [it.call.toolUseId] : [])) : [];

  it("the code and the calls it made are calls of one group; it counts the calls, code that called nothing and code that threw", () => {
    const { rows } = buildRows(
      input([
        userNode("u1", 1, "go"),
        asstNode("a1", 2, [
          ...repl("r1", [inner("v1", "Bash", { command: "ls" }), inner("v2", "Read", { file_path: "/a/b.rs" }, true)]),
          ...repl("r2", [], true),
        ]),
      ]),
    );
    const [g] = groups(rows);
    expect(callIds(g!)).toEqual(["r1", "v1", "v2", "r2"]);
    expect(g!.callCount).toBe(2);
    expect(g!.failedCount).toBe(2);
    const calls = g!.items.flatMap((it) => (it.type === "call" ? [it.call] : []));
    expect(describeCalls(calls)).toEqual({
      text: "读取 1 个文件、运行 1 条命令、执行 1 段代码",
      targets: ["ls", "b.rs"],
    });
  });

  it("a call the code made is found and opened as a row of its own", () => {
    const g = buildRows(
      input([userNode("u1", 1, "go"), asstNode("a1", 2, repl("r1", [inner("v1", "Bash", { command: "ls" })]))]),
    );
    expect(g.toolNode.get("v1")).toBe("a1");
    expect(rowIndexOf(g, "a1", "v1")).toBe(rowIndexOf(g, "a1", "r1"));
    expect(locate(g, "a1", "v1")?.expand).toEqual(["fold:u1", "group:a1|r1", "tool:a1|v1"]);
  });

  it("a question the code asked is a card of its own; code that only asked has no row, its card carries it", () => {
    const ask = inner("q1", "AskUserQuestion", { questions: [] });
    const g = buildRows(
      input([
        userNode("u1", 1, "go"),
        asstNode("a1", 2, repl("r1", [ask])),
        asstNode("a2", 3, repl("r2", [inner("v1", "Bash", { command: "ls" }), { ...ask, toolUseId: "q2" }])),
      ]),
    );
    const asks = g.rows.filter((r): r is Extract<Row, { kind: "ask" }> => r.kind === "ask");
    expect(asks.map((r) => [r.call.toolUseId, r.code?.toolUseId])).toEqual([
      ["q1", "r1"],
      ["q2", undefined],
    ]);
    // Its code opens on the card (a hit on its return value lands there).
    expect(rowIndexOf(g, "a1", "r1")).toBe(g.rows.indexOf(asks[0]!));
    expect(locate(g, "a1", "r1")?.expand).toContain("tool:a1|r1");
    // Code that did something else keeps its line, before the calls; the question still has its own card after them.
    expect(g.rows.filter((r) => r.kind === "group").map(callIds)).toEqual([["r2", "v1"]]);
  });
});
