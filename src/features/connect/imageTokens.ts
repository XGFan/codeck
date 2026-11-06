// Pasted images sit in the prompt where they were pasted, as tokens: in the textarea a short run of text — an
// invisible start mark, an ideographic space (room for the icon the mirror draws), 「图片 n」, a thin space, an
// invisible end mark — that the copy of the text under the (then clear) textarea draws as one block. The tokens are
// numbered in text order, and the images list follows them: token n is images[n - 1].

import type { DraftImage } from "./drafts";

/** INVISIBLE TIMES / INVISIBLE SEPARATOR: zero-width, not typed by hand, so they mark a token's ends. */
export const OPEN = "⁢";
export const CLOSE = "⁣";
const LABEL = "　图片 ";

export const token = (n: number) => `${OPEN}${LABEL}${n} ${CLOSE}`;

const TOKEN_RE = /⁢　图片 (\d+) ⁣/g;

export interface Tok {
  start: number;
  end: number;
  n: number;
}

export function tokensIn(text: string): Tok[] {
  return [...text.matchAll(TOKEN_RE)].map((m) => ({ start: m.index, end: m.index + m[0].length, n: Number(m[1]) }));
}

export const hasTokens = (text: string) => text.includes(OPEN);

/**
 * The text with whole tokens only (what is left of a broken one dropped), renumbered in text order, and the images
 * in that order: an image without a token goes, a token without an image (or a second copy of one) goes.
 */
export function normalize(text: string, images: DraftImage[]): { text: string; images: DraftImage[] } {
  const out: DraftImage[] = [];
  const used = new Set<number>();
  let s = "";
  let at = 0;
  for (const t of tokensIn(text)) {
    s += stray(text.slice(at, t.start));
    at = t.end;
    const img = images[t.n - 1];
    if (!img || used.has(t.n)) continue;
    used.add(t.n);
    out.push(img);
    s += token(out.length);
  }
  s += stray(text.slice(at));
  return { text: s, images: out };
}

/** Text between tokens, without the pieces of a broken one: its marks and what of 「　图片 n」 clings to them. */
const stray = (s: string) =>
  s.includes(OPEN) || s.includes(CLOSE) ? s.replace(/⁢[　图片 \d ]*|[　图片 \d ]*⁣/g, "") : s;

/** The token `pos` falls strictly inside, if any. */
export function tokenAt(text: string, pos: number): Tok | null {
  return tokensIn(text).find((t) => pos > t.start && pos < t.end) ?? null;
}

/** A caret inside a token goes to its end the way it moved (`dir` > 0: forward), else to the nearer end. */
export function snap(text: string, pos: number, dir: number): number {
  const t = tokenAt(text, pos);
  if (!t) return pos;
  if (dir > 0) return t.end;
  if (dir < 0) return t.start;
  return pos - t.start <= t.end - pos ? t.start : t.end;
}

/** A selection covering part of a token covers all of it. */
export function widen(text: string, start: number, end: number): [number, number] {
  let s = start;
  let e = end;
  for (const t of tokensIn(text)) {
    if (s > t.start && s < t.end) s = t.start;
    if (e > t.start && e < t.end) e = t.end;
  }
  return [s, e];
}

/** The token ending at `pos` (⌫ takes it whole) / starting at `pos` (Delete). */
export const tokenBefore = (text: string, pos: number) => tokensIn(text).find((t) => t.end === pos) ?? null;
export const tokenAfter = (text: string, pos: number) => tokensIn(text).find((t) => t.start === pos) ?? null;

/** For a reader: each token as 「[图片 n]」. */
export const plain = (text: string) => text.replace(TOKEN_RE, "[图片 $1]");

/** The words alone, tokens dropped (what decides a slash command, a `!` command, a blank box). */
export const withoutTokens = (text: string) => text.replace(TOKEN_RE, "");

export type Part = { text: string } | { image: string };

/** What `prompt` / `steer` send: the text between tokens and each token's image path, in order. */
export function toParts(text: string, images: DraftImage[]): Part[] {
  const parts: Part[] = [];
  let at = 0;
  for (const t of tokensIn(text)) {
    const before = text.slice(at, t.start);
    if (before) parts.push({ text: before });
    const img = images[t.n - 1];
    if (img) parts.push({ image: img.path });
    at = t.end;
  }
  const rest = text.slice(at);
  if (rest) parts.push({ text: rest });
  return parts;
}

/** `text`'s tokens numbered from `offset + 1` on (text pasted or put back after other images). */
export const shiftTokens = (text: string, offset: number) =>
  offset ? text.replace(TOKEN_RE, (_, n: string) => token(Number(n) + offset)) : text;

/** A draft from before tokens (images above the box): its images go first, as they were sent. */
export function withTokens(text: string, images: DraftImage[]): string {
  if (images.length === 0 || hasTokens(text)) return text;
  return images.map((_, i) => token(i + 1)).join("") + text;
}

/** `text` with `[start, end)` replaced by `insert`, the caret after it. */
export function splice(text: string, start: number, end: number, insert: string): { text: string; caret: number } {
  return { text: text.slice(0, start) + insert + text.slice(end), caret: start + insert.length };
}

/** The token `[start, end)` moved to `to` (an index in `text`, not inside a token); the caret after it. */
export function moveToken(text: string, tok: Tok, to: number): { text: string; caret: number } {
  if (to >= tok.start && to <= tok.end) return { text, caret: tok.end };
  const piece = text.slice(tok.start, tok.end);
  const without = text.slice(0, tok.start) + text.slice(tok.end);
  const at = to > tok.end ? to - piece.length : to;
  return splice(without, at, at, piece);
}
