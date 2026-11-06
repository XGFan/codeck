import { describe, expect, it } from "vitest";
import { queueText } from "./queue";

describe("queueText", () => {
  it("a message from another agent reads as its sender and first line", () => {
    const item =
      '<cross-session-message from="uds:/tmp/a.sock" hop-chain="8a7e" from-name="codeck-c6" from-mode="bypass">\nMy CoDeck.app is from 93fc585.\n\nMore.\n</cross-session-message>';
    expect(queueText(item)).toBe("codeck-c6：My CoDeck.app is from 93fc585.");
    expect(queueText('<agent-message from="line-a">\nDone.\n</agent-message>')).toBe("line-a：Done.");
  });

  it("a Teammate's message reads as its sender and preview; a task notification as its summary", () => {
    expect(queueText('<teammate-message teammate_id="gui" summary="GUI done">\nbody\n</teammate-message>')).toBe(
      "gui：GUI done",
    );
    expect(
      queueText(
        "<task-notification>\n<task-id>b1</task-id>\n<summary>Background command done</summary>\n</task-notification>",
      ),
    ).toBe("后台任务：Background command done");
  });

  it("anything else stays as typed", () => {
    expect(queueText("做完再跑一下测试")).toBe("做完再跑一下测试");
  });
});
