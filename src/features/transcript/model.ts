// Presentation-only shaping of ccrpc blocks: pairs each tool_use with its tool_result and turns the Main Line
// (or a Subagent Run's blocks) into rows — user bubbles, text, thinking, tool calls, tool groups, Agent cards.

import type { Block, TurnEndEvent } from "@/protocol";
import type { Item } from "@/state/conn";

/** ccrpc cuts block text at 100KB and appends this marker (ccrpc `transcript.rs`). */
export const TRUNCATED_MARK = "…[truncated]";

export interface ToolResult {
  content: string;
  isError: boolean;
  persisted: { path: string; size: number } | null;
  /** `content` was cut by ccrpc; the full text comes from `get_tool_result`. */
  truncated: boolean;
}

export interface ToolCall {
  id: string;
  name: string;
  input: Record<string, unknown>;
  result: ToolResult | null;
  /** No result and none will come (the turn ended, or a later prompt started). */
  unfinished: boolean;
}

/** claude's record of an interruption (Esc, a denial that stops the turn): part of that turn. */
export const isInterruption = (text: string) => text.startsWith("[Request interrupted by user");

/** Calls rendered as a card that opens the Subagent / Workflow panel. */
export const isAgentCall = (name: string) => name === "Agent" || name === "Task" || name === "Workflow";

export type GroupItem = { type: "call"; call: ToolCall } | { type: "thinking"; key: string; text: string };

export type Row =
  | { kind: "user"; key: string; text: string; queued: boolean }
  | { kind: "text"; key: string; text: string }
  | { kind: "thinking"; key: string; text: string }
  | { kind: "tool"; key: string; call: ToolCall }
  | {
      kind: "group";
      key: string;
      items: GroupItem[];
      /** Tool names with counts, most frequent first. */
      counts: [string, number][];
      callCount: number;
      failedCount: number;
    }
  | { kind: "agent"; key: string; call: ToolCall }
  /** A tool_result whose tool_use is not loaded (older page). */
  | { kind: "result"; key: string; toolUseId: string; result: ToolResult }
  | { kind: "turn_end"; key: string; ev: TurnEndEvent };

export interface Turn {
  key: string;
  rows: Row[];
}

const asInput = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};

export function toResult(b: Extract<Block, { kind: "tool_result" }>): ToolResult {
  return {
    content: b.content,
    isError: b.is_error,
    persisted: b.persisted ?? null,
    truncated: b.content.endsWith(TRUNCATED_MARK),
  };
}

/** tool_use id → its result, over every block given. */
export function resultsOf(blocks: Iterable<Block>): Map<string, ToolResult> {
  const m = new Map<string, ToolResult>();
  for (const b of blocks) if (b.kind === "tool_result") m.set(b.tool_use_id, toResult(b));
  return m;
}

const keyOf = (b: Block) => `${b.uuid}:${b.index}`;

/**
 * Rows of one block run. `results` pairs tool_use ids with results (may span more blocks than given);
 * `finished` marks calls still without a result as unfinished.
 */
export function buildRows(blocks: Block[], results: Map<string, ToolResult> = resultsOf(blocks), finished = false): Row[] {
  const rows: Row[] = [];
  const uses = new Set<string>();
  for (const b of blocks) if (b.kind === "tool_use") uses.add(b.id);

  let group: { items: GroupItem[]; calls: ToolCall[] } | null = null;
  let held: GroupItem[] = [];

  const flush = () => {
    const g = group;
    group = null;
    if (g) {
      if (g.calls.length === 1) {
        const call = g.calls[0]!;
        rows.push({ kind: "tool", key: `tool:${call.id}`, call });
      } else {
        const counts = new Map<string, number>();
        for (const c of g.calls) counts.set(c.name, (counts.get(c.name) ?? 0) + 1);
        rows.push({
          kind: "group",
          key: `group:${g.calls[0]!.id}`,
          items: g.items,
          counts: [...counts.entries()].sort((a, b) => b[1] - a[1]),
          callCount: g.calls.length,
          failedCount: g.calls.filter((c) => c.result?.isError).length,
        });
      }
    }
    for (const t of held) if (t.type === "thinking") rows.push({ kind: "thinking", key: t.key, text: t.text });
    held = [];
  };

  for (const b of blocks) {
    switch (b.kind) {
      case "user":
        flush();
        rows.push({ kind: "user", key: keyOf(b), text: b.text, queued: b.queued === true });
        break;
      case "text":
        if (!b.text.trim()) break;
        flush();
        rows.push({ kind: "text", key: keyOf(b), text: b.text });
        break;
      case "thinking":
        if (!b.text.trim()) break;
        if (group) held.push({ type: "thinking", key: keyOf(b), text: b.text });
        else rows.push({ kind: "thinking", key: keyOf(b), text: b.text });
        break;
      case "tool_use": {
        const result = results.get(b.id) ?? null;
        const call: ToolCall = { id: b.id, name: b.name, input: asInput(b.input), result, unfinished: !result && finished };
        if (isAgentCall(b.name)) {
          flush();
          rows.push({ kind: "agent", key: `agent:${b.id}`, call });
        } else if (group) {
          group.items.push(...held, { type: "call", call });
          held = [];
          group.calls.push(call);
        } else {
          group = { items: [{ type: "call", call }], calls: [call] };
        }
        break;
      }
      case "tool_result":
        if (uses.has(b.tool_use_id)) break;
        flush();
        rows.push({ kind: "result", key: keyOf(b), toolUseId: b.tool_use_id, result: toResult(b) });
        break;
    }
  }
  flush();
  return rows;
}

/**
 * Main Line items grouped by turn: a (non-queued) user block starts a turn, and so does anything after a
 * `turn_end`. Calls without a result are unfinished once their turn has ended or a later turn exists.
 */
export function buildTurns(items: Item[]): Turn[] {
  const results = resultsOf(items.flatMap((i) => (i.kind === "block" ? [i.block] : [])));
  const segs: { key: string; blocks: Block[]; end: TurnEndEvent | null; endKey: string }[] = [];
  let cur: (typeof segs)[number] | null = null;
  for (const it of items) {
    if (it.kind === "turn_end") {
      if (!cur) segs.push((cur = { key: it.key, blocks: [], end: null, endKey: "" }));
      cur.end = it.ev;
      cur.endKey = it.key;
      cur = null;
      continue;
    }
    const b = it.block;
    if (!cur || (b.kind === "user" && !b.queued && !isInterruption(b.text) && cur.blocks.length > 0)) {
      segs.push((cur = { key: it.key, blocks: [], end: null, endKey: "" }));
    }
    cur.blocks.push(b);
  }
  return segs.map((s, i) => {
    const rows = buildRows(s.blocks, results, s.end != null || i < segs.length - 1);
    if (s.end) rows.push({ kind: "turn_end", key: s.endKey, ev: s.end });
    return { key: s.key, rows };
  });
}

/** Finds the call `toolUseId` in the Main Line or in any Subagent / Workflow Run (nested calls). */
export function findCall(items: Item[], sub: Record<string, Block[]>, toolUseId: string): ToolCall | null {
  const lists: Block[][] = [items.flatMap((i) => (i.kind === "block" ? [i.block] : [])), ...Object.values(sub)];
  for (const blocks of lists) {
    const use = blocks.find((b): b is Extract<Block, { kind: "tool_use" }> => b.kind === "tool_use" && b.id === toolUseId);
    if (!use) continue;
    const res = blocks.find((b): b is Extract<Block, { kind: "tool_result" }> => b.kind === "tool_result" && b.tool_use_id === toolUseId);
    const result = res ? toResult(res) : null;
    return { id: use.id, name: use.name, input: asInput(use.input), result, unfinished: false };
  }
  return null;
}
