/** A piece of a typed prompt: plain text, an inline code span, or a fenced code block. */
export type PromptPart =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "block"; text: string; lang: string | null };

/** An opening fence at a line start: 3+ backticks, then an info string without backticks. */
const OPEN = /^ {0,3}(`{3,})[ \t]*([^`\n]*?)[ \t]*$/;

/**
 * Splits a prompt the user typed into text, `inline code` and ```fenced blocks```, the way markdown does for these two
 * only; everything else stays as typed. A fence runs to a line of at least as many backticks, or to the end; an inline
 * span is closed by the same number of backticks on the same line (an unmatched run stays text).
 */
export function promptParts(text: string): PromptPart[] {
  const out: PromptPart[] = [];
  const pushText = (s: string) => {
    if (!s) return;
    const last = out[out.length - 1];
    if (last?.kind === "text") last.text += s;
    else out.push({ kind: "text", text: s });
  };
  const lines = text.split("\n");
  let plain: string[] = [];
  const flush = () => {
    if (plain.length) inline(plain.join("\n"), pushText, out);
    plain = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const m = OPEN.exec(lines[i]);
    if (!m) {
      plain.push(lines[i]);
      continue;
    }
    const fence = m[1].length;
    const close = new RegExp(`^ {0,3}\`{${fence},}[ \\t]*$`);
    let j = i + 1;
    while (j < lines.length && !close.test(lines[j])) j++;
    flush();
    out.push({ kind: "block", text: lines.slice(i + 1, j).join("\n"), lang: m[2].split(/\s/)[0] || null });
    i = j;
  }
  flush();
  // A block is a box of its own: blank lines around it would only add space.
  return out.flatMap((p, i): PromptPart[] => {
    if (p.kind !== "text") return [p];
    let t = p.text;
    if (out[i - 1]?.kind === "block") t = t.replace(/^\n+/, "");
    if (out[i + 1]?.kind === "block") t = t.replace(/\n+$/, "");
    return t ? [{ ...p, text: t }] : [];
  });
}

/** Inline code spans within text with no fences. */
function inline(s: string, pushText: (s: string) => void, out: PromptPart[]) {
  const run = /`+/g;
  let at = 0;
  for (let m = run.exec(s); m; m = run.exec(s)) {
    const n = m[0].length;
    const lineEnd = s.indexOf("\n", run.lastIndex);
    const rest = s.slice(run.lastIndex, lineEnd < 0 ? s.length : lineEnd);
    const close = new RegExp(`(?<!\`)\`{${n}}(?!\`)`).exec(rest);
    if (!close) continue;
    let code = rest.slice(0, close.index);
    // As in markdown: one space padding on both sides is dropped (lets a span start or end with a backtick).
    if (code.length > 2 && code.startsWith(" ") && code.endsWith(" ") && code.trim()) code = code.slice(1, -1);
    if (!code.trim()) continue;
    pushText(s.slice(at, m.index));
    out.push({ kind: "code", text: code });
    at = run.lastIndex + close.index + n;
    run.lastIndex = at;
  }
  pushText(s.slice(at));
}
