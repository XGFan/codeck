import { describe, expect, it } from "vitest";
import { token } from "./imageTokens";
import type { QueueItem } from "./protocol";
import { queueText, rememberSteer, takenBack } from "./queue";

describe("queueText", () => {
  it("a message from elsewhere reads as its sender and first line; what was typed as it is", () => {
    const from = (kind: "session" | "agent", sender: string, text: string): QueueItem => ({
      kind,
      from: sender,
      text,
      editable: false,
    });
    expect(queueText(from("session", "codeck-c6", "\nMy CoDeck.app is from 93fc585.\n\nMore."))).toBe(
      "codeck-c6：My CoDeck.app is from 93fc585.",
    );
    expect(queueText(from("agent", "line-a", "Done."))).toBe("line-a：Done.");
    expect(queueText({ kind: "typed", from: null, text: "做完再\n跑一下测试", editable: true })).toBe(
      "做完再\n跑一下测试",
    );
  });
});

describe("taking the queue back", () => {
  const img = (n: number) => ({ path: `/img/${n}.png`, url: `data:image/png;base64,${n}` });

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
