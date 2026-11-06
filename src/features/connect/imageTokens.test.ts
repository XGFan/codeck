import { describe, expect, it } from "vitest";
import {
  moveToken,
  normalize,
  plain,
  shiftTokens,
  snap,
  toParts,
  token,
  tokenBefore,
  tokensIn,
  widen,
  withoutTokens,
  withTokens,
} from "./imageTokens";

const img = (p: string) => ({ path: `/i/${p}.png`, url: `data:${p}` });
const [A, B, C] = [img("a"), img("b"), img("c")];

describe("图片标记", () => {
  const text = `对比 ${token(1)} 和 ${token(2)}：没对齐`;

  it("splits into text and image parts in order, for ccrpc", () => {
    expect(toParts(text, [A, B])).toEqual([
      { text: "对比 " },
      { image: A.path },
      { text: " 和 " },
      { image: B.path },
      { text: "：没对齐" },
    ]);
    expect(plain(text)).toBe("对比 [图片 1] 和 [图片 2]：没对齐");
    expect(withoutTokens(text)).toBe("对比  和 ：没对齐");
  });

  it("renumbers in text order and reorders the images; a deleted token's image goes", () => {
    const swapped = `${token(2)} 然后 ${token(1)}`;
    expect(normalize(swapped, [A, B])).toEqual({ text: `${token(1)} 然后 ${token(2)}`, images: [B, A] });
    expect(normalize(`只剩 ${token(2)}`, [A, B])).toEqual({ text: `只剩 ${token(1)}`, images: [B] });
    // A copy of a token, or one without an image, goes.
    expect(normalize(`${token(1)}${token(1)}${token(3)}`, [A, B])).toEqual({ text: token(1), images: [A] });
  });

  it("drops what is left of a broken token, keeping the words around it", () => {
    const t = token(1);
    expect(normalize(`前${t.slice(0, 4)}后`, [A]).text).toBe("前后");
    expect(normalize(`前${t.slice(3)}后`, [A]).text).toBe("前后");
  });

  it("keeps the caret and the selection off a token's inside", () => {
    const s = `ab${token(1)}cd`;
    const [t] = tokensIn(s);
    expect(snap(s, t!.start + 2, 1)).toBe(t!.end);
    expect(snap(s, t!.start + 2, -1)).toBe(t!.start);
    expect(snap(s, t!.start + 1, 0)).toBe(t!.start);
    expect(snap(s, 1, 1)).toBe(1);
    expect(widen(s, 1, t!.start + 3)).toEqual([1, t!.end]);
    expect(tokenBefore(s, t!.end)).toEqual(t);
  });

  it("moves a token to a place between words", () => {
    const s = `ab${token(1)}cd`;
    const [t] = tokensIn(s);
    expect(moveToken(s, t!, s.length).text).toBe(`abcd${token(1)}`);
    expect(moveToken(s, t!, 0).text).toBe(`${token(1)}abcd`);
    expect(moveToken(s, t!, t!.start).text).toBe(s);
  });

  it("shifts pasted tokens past the images already there; an old draft's images go first", () => {
    expect(shiftTokens(`x${token(1)}`, 2)).toBe(`x${token(3)}`);
    expect(withTokens("看这个", [A, C])).toBe(`${token(1)}${token(2)}看这个`);
    expect(withTokens(text, [A, B])).toBe(text);
  });
});
