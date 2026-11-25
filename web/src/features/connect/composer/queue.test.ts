import { describe, expect, it } from "vitest";
import { token } from "../state/imageTokens";
import type { QueueItem } from "@/ipc/protocol/ccrpc";
import { queueText, rememberImages, takenBack } from "./queue";

describe("queueText", () => {
  it("a message from elsewhere reads as its sender and first line; what was typed as it is", () => {
    const from = (kind: "session" | "agent", sender: string, text: string): QueueItem => ({
      id: "q1",
      images: null,
      kind,
      from: sender,
      text,
      editable: false,
    });
    expect(queueText(from("session", "codeck-c6", "\nMy CoDeck.app is from 93fc585.\n\nMore."))).toBe(
      "codeck-c6：My CoDeck.app is from 93fc585.",
    );
    expect(queueText(from("agent", "line-a", "Done."))).toBe("line-a：Done.");
    expect(
      queueText({ id: "q2", kind: "typed", from: null, text: "做完再\n跑一下测试", editable: true, images: [] }),
    ).toBe("做完再\n跑一下测试");
  });
});

describe("taking the queue back", () => {
  const img = (n: number) => ({ path: `/img/${n}.png`, url: `data:image/png;base64,${n}` });
  const item = (id: string, text: string, images: string[] | null): QueueItem => ({
    id,
    kind: "typed",
    from: null,
    text,
    editable: true,
    images,
  });

  it("the items one per line, oldest first; an image of an unknown path is left out", () => {
    expect(takenBack([item("q1", "先跑测试", []), item("q2", "[Image #1] 再看看这个", ["/elsewhere/a.png"])])).toEqual({
      text: "先跑测试\n再看看这个",
      images: [],
    });
  });

  it("images sent from here come back by path, numbered in order across the items; ccrpc not knowing the paths leaves them out", () => {
    rememberImages([img(1), img(2), img(3)]);
    const back = takenBack([
      item("q1", "[Image #1][Image #2]两张", [img(2).path, img(3).path]),
      item("q2", "看 [Image #1]这里", [img(1).path]),
      item("q3", "没有图", []),
      item("q4", "[Image #1]", null),
    ]);
    expect(back.text).toBe(`${token(1)}${token(2)}两张\n看 ${token(3)}这里\n没有图`);
    expect(back.images).toEqual([img(2), img(3), img(1)]);
  });
});
