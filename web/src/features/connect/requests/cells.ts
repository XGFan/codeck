/**
 * A preview (ASCII mockup) laid out on the terminal's grid: Claude Code counts CJK, other East Asian wide characters
 * and emoji as two columns, everything else as one. The monospace font has no CJK glyphs (the fallback's are 1em, not
 * two columns) nor many symbols (▾ ● ─ │), so every non-ASCII character (a grapheme: 👨‍💻 or é is one) gets a box of
 * its own width.
 */
export interface Run {
  text: string;
  /** Columns of a single character in a box of its own; 0 for text left as it is (ASCII, zero-width characters). */
  w: 0 | 1 | 2;
}

// East Asian Wide / Fullwidth ranges (Hangul Jamo, CJK and its punctuation, kana, Hangul, compatibility
// ideographs, vertical and fullwidth forms, the supplementary ideograph planes).
const WIDE =
  /^[\u1100-\u115f\u2e80-\u303e\u3041-\u33ff\u3400-\u4dbf\u4e00-\u9fff\ua000-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe30-\ufe4f\uff00-\uff60\uffe0-\uffe6\u{20000}-\u{3fffd}]/u;
// Emoji drawn as emoji (✅ 🚀 🇨🇳 👨‍💻; not ● or ▶, which are text unless followed by U+FE0F).
// (The `v` flag through the constructor: tsc accepts the literal only when targeting ES2024; WebKit has it since 17.)
const EMOJI = new RegExp("^\\p{RGI_Emoji}$", "v");
const ASCII = /^[ -~\t\r\n]+$/;
const ZERO = /^[\p{Cf}\p{Mn}\p{Me}]+$/u;

const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" });

/** `text` as runs: ASCII stretches as they are (`w: 0`), each other grapheme alone with its column count. */
export function cells(text: string): Run[] {
  const runs: Run[] = [];
  let plain = "";
  for (const { segment: g } of graphemes.segment(text)) {
    if (ASCII.test(g) || ZERO.test(g)) {
      plain += g;
      continue;
    }
    if (plain) runs.push({ text: plain, w: 0 });
    plain = "";
    runs.push({ text: g, w: WIDE.test(g) || EMOJI.test(g) ? 2 : 1 });
  }
  if (plain) runs.push({ text: plain, w: 0 });
  return runs;
}
