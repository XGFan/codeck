// Folding by turn (Codex style): each turn's process — the rows before its answer that `buildRows` gave a `fold`
// key — shows as one line, 「已处理 1 分 50 秒 · 4 段说明、5 次工具调用」, opened by a click; the answer stays out.

import type { Grouping, Row } from "./grouping";

const rowTime = (r: Row | undefined): number | null => {
  if (!r) return null;
  if (r.kind === "group") return r.items[r.items.length - 1]?.node.timestampMs ?? null;
  return "node" in r ? (r.node.timestampMs ?? null) : null;
};

/** Tool calls a run of rows holds (Subagent / Workflow / question cards count as calls). */
function callCount(rows: Row[]): number {
  let n = 0;
  for (const r of rows) {
    if (r.kind === "tool" || r.kind === "subagent" || r.kind === "workflow" || r.kind === "ask") n++;
    else if (r.kind === "group") n += r.callCount;
    else if (r.kind === "parallel") n += r.cards.length;
  }
  return n;
}

/** Whether a fold is open: as the user left it (`expanded`), else only the running last turn's. */
export const foldOpen = (g: Grouping, expanded: Record<string, boolean>, key: string) => expanded[key] ?? key === g.liveFold;

/**
 * `g` with each folded turn's process replaced by its fold row (kept after it when open). Index entries of the
 * rows a closed fold hides point at the fold row.
 */
export function foldTurns(g: Grouping, expanded: Record<string, boolean>): Grouping {
  const rows = g.rows;
  if (!rows.some((r) => r.fold)) return g;
  const out: Row[] = [];
  const remap: number[] = new Array(rows.length);
  let i = 0;
  while (i < rows.length) {
    const key = rows[i]!.fold;
    if (!key) {
      remap[i] = out.length;
      out.push(rows[i++]!);
      continue;
    }
    let end = i;
    while (end < rows.length && rows[end]!.fold === key) end++;
    const proc = rows.slice(i, end);
    const open = foldOpen(g, expanded, key);
    const prompt = rows[i - 1];
    const start = rowTime(prompt);
    // Until the answer starts (or the last of the process, while there is none).
    const last = rowTime(rows[end] && rows[end]!.turn === prompt?.turn ? rows[end] : proc[proc.length - 1]);
    const at = out.length;
    out.push({
      kind: "fold",
      key,
      turn: rows[i]!.turn,
      open,
      ms: start != null && last != null && last >= start ? last - start : null,
      texts: proc.filter((r) => r.kind === "text" && r.text.trim()).length,
      calls: callCount(proc),
    });
    for (let j = i; j < end; j++) {
      remap[j] = open ? out.length : at;
      if (open) out.push(rows[j]!);
    }
    i = end;
  }
  return { ...g, rows: out, index: new Map([...g.index].map(([k, v]) => [k, remap[v] ?? v])) };
}
