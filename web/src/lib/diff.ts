import { structuredPatch } from "diff";

export type DiffRowKind = "ctx" | "add" | "del" | "gap";

export interface DiffRow {
  kind: DiffRowKind;
  oldNo?: number;
  newNo?: number;
  text: string;
}

export interface DiffResult {
  rows: DiffRow[];
  added: number;
  removed: number;
}

export interface Hunk {
  oldStart: number;
  newStart: number;
  lines: string[];
}

/** Flattens hunks (jsdiff / Claude Code `structuredPatch` shape) into numbered rows; hunks are separated by a `gap` row. */
export function rowsFromHunks(hunks: Hunk[]): DiffResult {
  const rows: DiffRow[] = [];
  let added = 0;
  let removed = 0;
  hunks.forEach((h, i) => {
    if (i > 0) rows.push({ kind: "gap", text: "" });
    let o = h.oldStart;
    let n = h.newStart;
    for (const line of h.lines) {
      const mark = line[0];
      const text = line.slice(1);
      if (mark === "+") {
        rows.push({ kind: "add", newNo: n++, text });
        added++;
      } else if (mark === "-") {
        rows.push({ kind: "del", oldNo: o++, text });
        removed++;
      } else if (mark === "\\") {
        continue; // "\ No newline at end of file"
      } else {
        rows.push({ kind: "ctx", oldNo: o++, newNo: n++, text });
      }
    }
  });
  return { rows, added, removed };
}

/** Diff of two strings with 3 lines of context. */
export function diffStrings(oldStr: string, newStr: string): DiffResult {
  const p = structuredPatch("a", "b", oldStr, newStr, undefined, undefined, { context: 3 });
  return rowsFromHunks(p.hunks);
}

/** Concatenates several diffs (MultiEdit), separated by gap rows. */
export function joinDiffs(parts: DiffResult[]): DiffResult {
  const rows: DiffRow[] = [];
  parts.forEach((p, i) => {
    if (i > 0 && p.rows.length) rows.push({ kind: "gap", text: "" });
    rows.push(...p.rows);
  });
  return { rows, added: parts.reduce((s, p) => s + p.added, 0), removed: parts.reduce((s, p) => s + p.removed, 0) };
}
