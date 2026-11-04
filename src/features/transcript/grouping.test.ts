import { describe, expect, it } from "vitest";
import type { Node, SubagentRun } from "@/ipc/bindings";
import { asstNode, call, otherNode, text, thinking, userNode } from "@/mocks/data/builders";
import { richMain, richSubagents, richWorkflows } from "@/mocks/data/rich";
import { buildRows, describeCalls, type GroupingInput, isGroupOpen, locate, rowIndexOf, type Row } from "./grouping";

const ok = (t = "ok") => ({ text: t });
const input = (nodes: Node[], o: Partial<GroupingInput> = {}): GroupingInput => ({
  nodes,
  branchPoints: [],
  subagents: [],
  workflows: [],
  inherited: null,
  orphanSubagentIds: [],
  showHidden: false,
  inheritedExpanded: false,
  ...o,
});
const kinds = (rows: Row[]) => rows.map((r) => r.kind);
const groups = (rows: Row[]) => rows.filter((r): r is Extract<Row, { kind: "group" }> => r.kind === "group");

describe("tool-call groups", () => {
  it("merges consecutive calls across assistant messages and splits on assistant text", () => {
    const { rows } = buildRows(
      input([
        userNode("u1", 1, "go"),
        asstNode("a1", 2, [text("look"), call("t1", "Bash", {}, ok()), call("t2", "Read", {}, ok())]),
        asstNode("a2", 3, [call("t3", "Bash", {}, ok())]),
        asstNode("a3", 4, [text("now edit"), call("t4", "Edit", {}, ok())]),
      ]),
    );
    expect(kinds(rows)).toEqual(["prompt", "text", "group", "text", "tool"]);
    const [g] = groups(rows);
    expect(g!.callCount).toBe(3);
    expect(g!.counts).toEqual([
      ["Bash", 2],
      ["Read", 1],
    ]);
    expect(g!.key).toBe("group:a1|t1");
    // Only the first row of the run carries the role line.
    expect(rows.filter((r) => "role" in r && r.role).length).toBe(1);
  });

  it("a human prompt starts a new turn and ends the group", () => {
    const { rows } = buildRows(
      input([
        userNode("u1", 1, "a"),
        asstNode("a1", 2, [call("t1", "Bash", {}, ok()), call("t2", "Bash", {}, ok())]),
        userNode("u2", 3, "b"),
        asstNode("a2", 4, [call("t3", "Bash", {}, ok()), call("t4", "Bash", {}, ok())]),
      ]),
    );
    expect(kinds(rows)).toEqual(["prompt", "group", "prompt", "group"]);
    expect(rows.map((r) => r.turn)).toEqual([0, 0, 1, 1]);
  });

  it("thinking between calls stays inside the group; leading thinking stands alone", () => {
    const { rows } = buildRows(
      input([
        asstNode("a1", 1, [thinking("plan"), call("t1", "Bash", {}, ok())]),
        asstNode("a2", 2, [thinking("hmm"), call("t2", "Read", {}, ok())]),
      ]),
    );
    expect(kinds(rows)).toEqual(["thinking", "group"]);
    expect(groups(rows)[0]!.items.map((i) => i.type)).toEqual(["call", "thinking", "call"]);
  });

  it("AskUserQuestion stands alone between groups and needs no expansion to reveal", () => {
    const g = buildRows(
      input([
        asstNode("a1", 1, [
          call("t1", "Bash", {}, ok()),
          call("t2", "Read", {}, ok()),
          call("q1", "AskUserQuestion", { questions: [] }, ok()),
          call("t3", "Bash", {}, ok()),
          call("t4", "Read", {}, ok()),
        ]),
      ]),
    );
    expect(kinds(g.rows)).toEqual(["group", "ask", "group"]);
    expect(rowIndexOf(g, "a1", "q1")).toBe(1);
    expect(locate(g, "a1", "q1")?.expand).toEqual([]);
  });

  it("locate opens the group holding a thinking block", () => {
    const g = buildRows(
      input([
        asstNode("a1", 1, [call("t1", "Bash", {}, ok())]),
        asstNode("a2", 2, [thinking("hmm"), call("t2", "Read", {}, ok())]),
      ]),
    );
    expect(locate(g, "a2", null, { thinking: true })?.expand).toEqual(["group:a1|t1", "thinking:a2"]);
    expect(locate(g, "a1", null, { thinking: true })?.expand).toEqual(["thinking:a1"]);
  });

  it("a lone call that grows into a group keeps the user's expansion visible", () => {
    const one = buildRows(input([asstNode("a1", 1, [call("t1", "Bash", {}, ok())])]));
    expect(kinds(one.rows)).toEqual(["tool"]);
    const expanded = { "tool:a1|t1": true };
    const two = buildRows(input([asstNode("a1", 1, [call("t1", "Bash", {}, ok())]), asstNode("a2", 2, [call("t2", "Read", {}, ok())])]));
    const [g] = groups(two.rows);
    expect(isGroupOpen(g!, expanded)).toBe(true);
    expect(isGroupOpen(g!, {})).toBe(false);
    // An explicit choice on the group wins.
    expect(isGroupOpen(g!, { ...expanded, [g!.key]: false })).toBe(false);
  });

  it("counts failed calls but keeps the group collapsed by default", () => {
    const rich = buildRows(input(richMain, { subagents: richSubagents, workflows: richWorkflows }));
    const failedGroup = groups(rich.rows).find((g) => g.items.some((i) => i.type === "call" && i.call.toolUseId === "tu_bash_fail"));
    expect(failedGroup?.failedCount).toBe(1);
    expect(isGroupOpen(failedGroup!, {})).toBe(false);
    expect(groups(rich.rows).filter((g) => g !== failedGroup).every((g) => g.failedCount === 0)).toBe(true);
  });
});

describe("subagent and workflow cards", () => {
  const rich = buildRows(input(richMain, { subagents: richSubagents, workflows: richWorkflows, orphanSubagentIds: ["ag-orphan"] }));

  it("groups parallel Agent calls of one message and renders the workflow as its own card", () => {
    const par = rich.rows.find((r): r is Extract<Row, { kind: "parallel" }> => r.kind === "parallel");
    expect(par?.cards.map((c) => c.run.agentId)).toEqual(["ag-sync1", "ag-async1"]);
    const wfAt = rich.rows.findIndex((r) => r.kind === "workflow");
    expect(rich.rows[wfAt - 1]).toBe(par);
    const after = rich.rows[wfAt + 1] as Extract<Row, { kind: "group" }>;
    expect(after.kind).toBe("group");
    expect(after.items.map((i) => (i.type === "call" ? i.call.toolUseId : null))).toEqual(["tu_mcp", "tu_bg_build", "tu_artifact"]);
  });

  it("links the task notification and lists orphans at the end", () => {
    expect(rich.rows.find((r) => r.kind === "notification")?.key).toBe("n-notify");
    expect(rich.toolNode.get("tu_agent_tests")).toBe("n12");
    expect(kinds(rich.rows).slice(-2)).toEqual(["orphanHeader", "orphan"]);
  });

  it("an Agent call without a linked run renders as a plain tool call", () => {
    const { rows } = buildRows(input([asstNode("a1", 1, [call("t1", "Agent", { prompt: "x" }, ok(), { subagentId: "missing" })])]));
    expect(kinds(rows)).toEqual(["tool"]);
  });

  it("subagent scope shows the task prompt card and the final result", () => {
    const agent: SubagentRun = richSubagents[0]!;
    const { rows } = buildRows(
      input([userNode("s-u1", 1, "task"), asstNode("s-a1", 2, [text("done")])], { agent }),
    );
    expect(kinds(rows)).toEqual(["taskPrompt", "text", "finalResult"]);
  });
});

describe("compact and hidden", () => {
  it("merges the compact boundary with the following summary", () => {
    const { rows, compactKeyOf } = buildRows(input(richMain));
    const c = rows.find((r) => r.kind === "compact");
    expect(c).toMatchObject({ boundary: { id: "n-cb" }, summary: { id: "n-cs" } });
    expect(compactKeyOf.get("n-cs")).toBe("compact:n-cs");
  });

  it("skips hidden nodes unless shown", () => {
    const nodes = [userNode("u1", 1, "a"), otherNode("h1", 2, { kind: "attachment", attachmentType: "x", text: "t" }, { hidden: true })];
    expect(buildRows(input(nodes)).rows).toHaveLength(1);
    expect(buildRows(input(nodes, { showHidden: true })).rows).toHaveLength(2);
  });
});

describe("fork-inherited prefix", () => {
  const nodes = [
    userNode("i1", 1, "old", { inherited: true }),
    asstNode("i2", 2, [call("ti1", "Bash", {}, ok()), call("ti2", "Bash", {}, ok())], { inherited: true }),
    userNode("n1", 3, "new"),
    asstNode("n2", 4, [text("reply")]),
  ];
  const range = { originSessionId: "origin", originTitle: "Origin", lastInheritedId: "i2", count: 2 };

  it("collapses into a banner followed by the new-content divider", () => {
    const g = buildRows(input(nodes, { inherited: range }));
    expect(kinds(g.rows)).toEqual(["inheritedBanner", "inheritedDivider", "prompt", "text"]);
    expect(g.rows[0]).toMatchObject({ count: 2, expanded: false });
    expect(rowIndexOf(g, "i2", "ti1")).toBe(0);
  });

  it("expands in place between banner and divider", () => {
    const g = buildRows(input(nodes, { inherited: range, inheritedExpanded: true }));
    expect(kinds(g.rows)).toEqual(["inheritedBanner", "prompt", "group", "inheritedDivider", "prompt", "text"]);
  });

  it("locate returns the keys that reveal a call inside the collapsed prefix", () => {
    const full = buildRows(input(nodes, { inherited: range, inheritedExpanded: true }));
    expect(locate(full, "i2", "ti2")?.expand).toEqual(["inherited", "group:i2|ti1", "tool:i2|ti2"]);
    expect(locate(full, "nope")).toBeNull();
  });
});

describe("branch heads", () => {
  it("attaches the branch point to the selected head prompt and to assistant heads", () => {
    const bp = (head: string) => ({ anchorKey: "x", selectedHeadId: head, options: [] });
    const nodes = [userNode("u1", 1, "a"), asstNode("a1", 2, [text("r1")]), asstNode("a2", 3, [text("r2")])];
    const { rows } = buildRows(input(nodes, { branchPoints: [bp("u1"), bp("a2")] }));
    expect(rows[0]).toMatchObject({ kind: "prompt", branch: { selectedHeadId: "u1" } });
    // An assistant head starts a new run so its role line (with the switcher) is shown.
    expect(rows[2]).toMatchObject({ kind: "text", role: { branch: { selectedHeadId: "a2" } } });
  });
});

describe("回答与工具组的摘要", () => {
  const { rows } = buildRows(
    input([
      userNode("u1", 1_000, "go"),
      asstNode("a1", 2_000, [text("先看看"), call("t1", "Read", { file_path: "/r/src/a.rs" }, ok()), call("t2", "Edit", { file_path: "/r/src/a.rs" }, ok())]),
      asstNode("a2", 3_000, [call("t3", "Write", { file_path: "/r/src/a.rs" }, ok()), call("t4", "Bash", { command: "cargo test\n--all" }, { text: "x", isError: true }), call("t5", "mcp__gh__list", {}, ok())]),
      asstNode("a3", 65_000, [text("改好了。"), text("测试有一处失败。")]),
      userNode("u2", 70_000, "thanks"),
      asstNode("a4", 71_000, [text("不客气")]),
    ]),
  );

  it("the text after a turn's last work is its answer: its last row carries all of it, for copying, and its time", () => {
    expect(kinds(rows)).toEqual(["prompt", "text", "group", "text", "text", "prompt", "text"]);
    const answers = rows.filter((r): r is Extract<Row, { kind: "text" }> => r.kind === "text" && !!r.answer);
    expect(answers.map((r) => r.answer)).toEqual([
      { text: "改好了。\n\n测试有一处失败。", timeMs: 65_000 },
      { text: "不客气", timeMs: 71_000 },
    ]);
  });

  it("text before or between a turn's work is process text; the answer's rows are the answer", () => {
    const texts = rows.filter((r): r is Extract<Row, { kind: "text" }> => r.kind === "text");
    expect(texts.map((r) => [r.text, r.layer])).toEqual([
      ["先看看", "process"],
      ["改好了。", "answer"],
      ["测试有一处失败。", "answer"],
      ["不客气", "answer"],
    ]);
  });

  it("a group says what its calls did and on what; edited files count once", () => {
    const [g] = groups(rows);
    const calls = g!.items.flatMap((it) => (it.type === "call" ? [it.call] : []));
    expect(describeCalls(calls)).toEqual({
      text: "读取 1 个文件、编辑 1 个文件、运行 1 条命令 等 5 次工具调用",
      targets: ["a.rs", "cargo test", "mcp__gh__list"],
    });
    expect(describeCalls(calls.filter((c) => c.name === "mcp__gh__list")).text).toBe("1 次工具调用");
  });
});
