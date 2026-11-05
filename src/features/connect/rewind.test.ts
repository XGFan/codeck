import { describe, expect, it } from "vitest";
import type { BranchPoint, Node } from "@/ipc/bindings";
import { codeLine, mainLinePrompts, rewindable, rewoundLabel } from "./rewind";

const prompt = (id: string, origin: Extract<Node["body"], { kind: "userPrompt" }>["origin"] = { kind: "human" }) =>
  ({
    id,
    timestampMs: null,
    hidden: false,
    inherited: false,
    body: { kind: "userPrompt", text: id, images: [], origin },
  }) as Node;
const reply = (id: string) =>
  ({
    id,
    timestampMs: null,
    hidden: false,
    inherited: false,
    body: { kind: "assistant", messageId: null, model: null, blocks: [], usage: null, isApiError: false },
  }) as Node;
const point = (selected: string, main: string): BranchPoint => ({
  anchorKey: "a",
  selectedHeadId: selected,
  options: [main, selected === main ? "other" : selected].map((headId) => ({
    headId,
    preview: headId,
    timestampMs: null,
    replyCount: 1,
    isMainLine: headId === main,
  })),
});

describe("rewind", () => {
  it("says what the confirm page says about the code", () => {
    expect(codeLine("The code will be unchanged.")).toBe("这条提示词及之后的对话会被移除；代码不变");
    expect(codeLine("The code will be restored -1 in w.txt.")).toBe(
      "这条提示词及之后的对话会被移除；代码会恢复：w.txt（−1）",
    );
    expect(codeLine("The code will be restored +3 -12 in src/a.rs and README.md.")).toBe(
      "这条提示词及之后的对话会被移除；代码会恢复：src/a.rs and README.md（+3 −12）",
    );
    // Not read from the page, or worded otherwise: the conversation part, or the sentence as it is.
    expect(codeLine(null)).toBe("这条提示词及之后的对话会被移除");
    expect(codeLine("The code will be partly restored.")).toBe("The code will be partly restored.");
  });

  it("offers the prompts and commands typed on the Main Line, before the one already rewound to", () => {
    const nodes = [
      prompt("p1"),
      reply("a1"),
      prompt("c1", { kind: "command", name: "/oh-my-claudecode:autopilot", args: "build it" }),
      prompt("o1", { kind: "commandOutput" }),
      prompt("b1", { kind: "command", name: "!", args: "echo bang" }),
      prompt("n1", { kind: "taskNotification", taskId: null, toolUseId: null, status: null, summary: null }),
      prompt("p2"),
      reply("a2"),
      prompt("p3"),
    ];
    // Command output and notifications are not typed: no 「回退」.
    expect(mainLinePrompts(nodes, [])).toEqual({ ids: ["p1", "c1", "b1", "p2", "p3"], wholeMain: true });
    // A branch point on the Main Line's own head changes nothing; another Branch's head ends the Main Line part.
    expect(mainLinePrompts(nodes, [point("p2", "p2")])).toEqual({
      ids: ["p1", "c1", "b1", "p2", "p3"],
      wholeMain: true,
    });
    expect(mainLinePrompts(nodes, [point("p2", "x2")])).toEqual({ ids: ["p1", "c1", "b1"], wholeMain: false });

    expect(rewindable(["p1", "p2", "p3"], null)).toEqual(["p1", "p2", "p3"]);
    expect(rewindable(["p1", "p2", "p3"], "p2")).toEqual(["p1"]);
    // Not on the Main Line any more: nothing is dimmed, every prompt is offered.
    expect(rewindable(["p1", "p2", "p3"], "gone")).toEqual(["p1", "p2", "p3"]);
  });

  it("says whether the code was restored above the rewound prompt", () => {
    expect(rewoundLabel(true)).toBe("已回退到这里，代码已恢复 · 发出新提示词后，下面这段成为另一个 Branch");
    expect(rewoundLabel(false)).toBe("已回退到这里 · 发出新提示词后，下面这段成为另一个 Branch");
  });
});
