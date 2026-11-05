import { describe, expect, it } from "vitest";
import { answerError, otherChosen, questionAnswers, togglePick } from "./answers";
import type { AskQuestion } from "./protocol";

describe("togglePick", () => {
  it("one answer: picking sets it alone, picking it again clears it", () => {
    expect(togglePick([], "A", false)).toEqual(["A"]);
    expect(togglePick(["A"], "B", false)).toEqual(["B"]);
    expect(togglePick(["A"], "A", false)).toEqual([]);
  });

  it("several answers: picking adds or removes it, keeping the others in order", () => {
    expect(togglePick(["A"], "B", true)).toEqual(["A", "B"]);
    expect(togglePick(["A", "B", "C"], "B", true)).toEqual(["A", "C"]);
  });
});

describe("questionAnswers", () => {
  const q = (question: string): AskQuestion => ({ question, options: [] });

  it("gives the written answer where 其他 is chosen and not blank, the options picked elsewhere", () => {
    const qs = [q("a"), q("b"), q("c")];
    expect(questionAnswers(qs, [["X"], [], ["Y"]], ["", "  写的  ", "   "], [false, true, true])).toEqual([
      { options: ["X"] },
      { other: "写的" },
      { options: ["Y"] },
    ]);
  });

  it("text written but set aside (an option picked since) does not count", () => {
    expect(otherChosen(["写的"], [false], 0)).toBe(false);
    expect(otherChosen(["写的"], [true], 0)).toBe(true);
  });
});

describe("answerError", () => {
  it("says an approval has no 总是允许 option, otherwise the error as given", () => {
    expect(answerError({ code: "not_offered", message: "x" })).toBe("这次审批没有“总是允许”选项");
    expect(answerError({ code: "timeout", message: "按键超时" })).toBe("作答失败：按键超时");
    expect(answerError({ code: "timeout", message: "" })).toBe("作答失败：");
    expect(answerError(undefined)).toBe("作答失败：");
  });
});
