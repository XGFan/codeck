import Anser from "anser";

export interface AnsiSpan {
  text: string;
  /** CSS color, e.g. `rgb(255, 85, 85)`. */
  fg?: string;
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

// eslint-disable-next-line no-control-regex
const ANSI_RE = /\u001b\[[0-9;?]*[A-Za-z]/g;

export const stripAnsi = (s: string): string => s.replace(ANSI_RE, "");

/** Pure black is invisible on the terminal background; lift it to a readable grey. */
const lift = (rgb: string): string => (rgb === "0, 0, 0" ? "rgb(110, 110, 118)" : `rgb(${rgb})`);

/** Parses SGR escapes into styled spans (other escape sequences are dropped). */
export function parseAnsi(text: string): AnsiSpan[] {
  if (!text.includes("\u001b")) return text ? [{ text }] : [];
  return Anser.ansiToJson(text, { json: true, remove_empty: true, use_classes: false }).map((e) => {
    const span: AnsiSpan = { text: stripAnsi(e.content) };
    if (e.fg) span.fg = lift(e.fg);
    if (e.bg) span.bg = `rgb(${e.bg})`;
    for (const d of e.decorations) {
      if (d === "bold") span.bold = true;
      else if (d === "dim") span.dim = true;
      else if (d === "italic") span.italic = true;
      else if (d === "underline") span.underline = true;
    }
    return span;
  });
}
