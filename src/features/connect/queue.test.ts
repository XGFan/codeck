import { describe, expect, it } from "vitest";
import { token } from "./imageTokens";
import { editable, queueText, rememberSteer, takenBack } from "./queue";

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

describe("taking the queue back", () => {
  const img = (n: number) => ({ path: `/img/${n}.png`, url: `data:image/png;base64,${n}` });

  it("only what was typed comes back, not a message from elsewhere", () => {
    expect(editable("做完再跑一下测试")).toBe(true);
    expect(editable("[Image #1]看这个")).toBe(true);
    expect(
      editable('<cross-session-message from="uds:/tmp/a.sock" from-name="c6">\nhi\n</cross-session-message>'),
    ).toBe(false);
    expect(editable('<agent-message from="line-a">\nDone.\n</agent-message>')).toBe(false);
  });

  it("the items one per line, oldest first; an image from elsewhere is left out", () => {
    expect(takenBack("c-plain", ["先跑测试", "[Image #1] 再看看这个"])).toEqual({
      text: "先跑测试\n再看看这个",
      images: [],
    });
  });

  it("a steer sent with images comes back as typed, its images too, numbered after the ones before it", () => {
    rememberSteer("c-img", { text: `看 ${token(1)} 这里`, images: [img(1)] });
    rememberSteer("c-img", { text: `${token(1)}${token(2)}两张`, images: [img(2), img(3)] });
    // ccrpc's items: the words with Claude Code's `[Image #n]` where the images were (its spacing, its numbers).
    const back = takenBack("c-img", ["[Image #2][Image #3]两张", "看 [Image #1]这里", "没有图"]);
    expect(back.text).toBe(`${token(1)}${token(2)}两张\n看 ${token(3)} 这里\n没有图`);
    expect(back.images).toEqual([img(2), img(3), img(1)]);
    // Taken: the same words with an image again (typed in the terminal) do not get them a second time.
    expect(takenBack("c-img", ["看 [Image #1]这里"])).toEqual({ text: "看 这里", images: [] });
    // Not another connection's; an item that was only an image is gone.
    rememberSteer("c-img2", { text: `看 ${token(1)} 这里`, images: [img(1)] });
    expect(takenBack("c-other", ["看 [Image #1]这里", "[Image #1]"])).toEqual({ text: "看 这里", images: [] });
  });
});
