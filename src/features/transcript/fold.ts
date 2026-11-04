// Folding by turn (Codex style): each turn's process — the rows before its answer that `buildRows` gave a `fold`
// key — shows as one line, 「已处理 1 分 50 秒 · 4 段说明、5 次工具调用」, opened by a click; the answer stays out.
// Opened, the process lists every call on its own line: the fold already hides it, so runs of calls are not
// grouped again (they still are where nothing folds, as in a Subagent's panel).

import { anchorKey, type Grouping, type Row } from "./grouping";

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

/** A group of a folded process as its own rows: each call and thinking in order, the group's role on the first. */
function ungroup(g: Extract<Row, { kind: "group" }>): Row[] {
  return g.items.map((it, k): Row => {
    const base = { turn: g.turn, fold: g.fold, role: k === 0 ? g.role : null };
    return it.type === "call"
      ? { ...base, kind: "tool", key: `${it.node.id}|${it.call.toolUseId}`, node: it.node, call: it.call }
      : { ...base, kind: "thinking", key: `${it.node.id}#${it.blockIndex}`, node: it.node, text: it.text };
  });
}

/**
 * `g` with each folded turn's process replaced by its fold row (followed by the process, groups split into their
 * calls, when open). Index entries of the rows a closed fold hides point at the fold row.
 */
export function foldTurns(g: Grouping, expanded: Record<string, boolean>): Grouping {
  const rows = g.rows;
  if (!rows.some((r) => r.fold)) return g;
  const out: Row[] = [];
  const remap: number[] = new Array(rows.length);
  /** Index keys (`node|call`) of the calls of split groups → their rows. */
  const split = new Map<string, number>();
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
      if (!open) continue;
      const r = rows[j]!;
      if (r.kind !== "group") {
        out.push(r);
        continue;
      }
      for (const row of ungroup(r)) {
        if (row.kind === "tool") split.set(anchorKey(row.node.id, row.call.toolUseId), out.length);
        out.push(row);
      }
    }
    i = end;
  }
  // A split group's calls point at their own rows; its nodes keep the group's place, now its first row.
  const index = new Map([...g.index].map(([k, v]) => [k, split.get(k) ?? remap[v] ?? v]));
  return { ...g, rows: out, index };
}
