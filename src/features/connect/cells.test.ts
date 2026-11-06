import { describe, expect, it } from "vitest";
import { cells } from "./cells";

describe("cells", () => {
  it("leaves ASCII stretches as they are and boxes each other character by its columns", () => {
    expect(cells("▾ codeck  +\n   ● 重新设计 2分")).toEqual([
      { text: "▾", w: 1 },
      { text: " codeck  +\n   ", w: 0 },
      { text: "●", w: 1 },
      { text: " ", w: 0 },
      { text: "重", w: 2 },
      { text: "新", w: 2 },
      { text: "设", w: 2 },
      { text: "计", w: 2 },
      { text: " 2", w: 0 },
      { text: "分", w: 2 },
    ]);
  });

  it("counts fullwidth punctuation, kana, Hangul and emoji as two columns, box drawing and text symbols as one", () => {
    const w = (s: string) => cells(s).map((r) => r.w);
    expect(w("（」カ한😀✅🚀")).toEqual([2, 2, 2, 2, 2, 2, 2]);
    expect(w("─│…●▶")).toEqual([1, 1, 1, 1, 1]);
  });

  it("boxes a grapheme as one: a ZWJ emoji, a flag, a letter with a combining mark; zero-width characters unboxed", () => {
    expect(cells("👨‍💻🇨🇳e\u0301")).toEqual([
      { text: "👨‍💻", w: 2 },
      { text: "🇨🇳", w: 2 },
      { text: "e\u0301", w: 1 },
    ]);
    expect(cells("a\u200bb")).toEqual([{ text: "a\u200bb", w: 0 }]);
  });
});
