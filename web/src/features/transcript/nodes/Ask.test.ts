import { describe, expect, it } from "vitest";
import { clarifyAnswers } from "./Ask";

describe("clarifyAnswers", () => {
  it("reads the answers and notes given before the user chose to chat", () => {
    const text = [
      "The user doesn't want to proceed with this tool use. To tell you how to proceed, the user said:",
      "The user wants to clarify these questions.",
      "",
      "    Questions asked:",
      '- "用哪个 Brand 做冒烟？(影响 "dryRun" 候选数)"',
      "  Answer: 你搞错重点了",
      "  User notes: 先看日志",
      '- "要不要限定范围?"',
      "  (No answer provided)",
      "",
      "Note: The user's next message may contain a correction or preference.",
    ].join("\n");
    expect(clarifyAnswers(text)).toEqual({
      answers: { '用哪个 Brand 做冒烟？(影响 "dryRun" 候选数)': "你搞错重点了" },
      annotations: { '用哪个 Brand 做冒烟？(影响 "dryRun" 候选数)': { notes: "先看日志" } },
    });
  });
});
