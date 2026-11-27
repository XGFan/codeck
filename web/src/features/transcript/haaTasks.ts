import type { HaaTask, Node } from "@/ipc/bindings";
import type { Grouping, Row } from "./grouping";

/**
 * Returns the id of the last non-hidden node with `timestampMs <= endedMs`, or `null` when no such node exists.
 */
export function anchorNodeId(nodes: readonly Node[], endedMs: number): string | null {
  for (let i = nodes.length - 1; i >= 0; i--) {
    const n = nodes[i]!;
    if (!n.hidden && n.timestampMs != null && n.timestampMs <= endedMs) return n.id;
  }
  return null;
}

function rowContainsNode(row: Row, nodeId: string): boolean {
  if (row.kind === "group") return row.items.some((it) => it.node.id === nodeId);
  if (row.kind === "compact") return row.boundary.id === nodeId || row.summary?.id === nodeId;
  if (row.kind === "apiRetry") return row.nodes.some((n) => n.id === nodeId);
  return "node" in row && row.node.id === nodeId;
}

function lastRowForNode(grouping: Grouping, nodeId: string): number | undefined {
  for (let i = grouping.rows.length - 1; i >= 0; i--) {
    if (rowContainsNode(grouping.rows[i]!, nodeId)) return i;
  }
  return grouping.index.get(nodeId);
}

/**
 * Finds the row index in `grouping` to place a task after: the last row containing the last non-hidden node with
 * `timestampMs <= endedMs` (falling back to earlier qualifying nodes if a node produced no row), or `null` to place
 * before the first row.
 */
function anchorRowIndex(grouping: Grouping, nodes: readonly Node[], endedMs: number): number | null {
  for (let i = nodes.length - 1; i >= 0; i--) {
    const n = nodes[i]!;
    if (n.hidden || n.timestampMs == null || n.timestampMs > endedMs) continue;
    const idx = lastRowForNode(grouping, n.id);
    if (idx !== undefined) return idx;
  }
  return null;
}

const taskRowKey = (order: number, task: HaaTask) =>
  `haa-task:${order}:${task.endedMs}:${task.sessionId ?? task.worker ?? ""}`;

/**
 * Inserts haa Worker Task cards at the top level of `grouping.rows` (after `foldTurns`, so cards never fold into
 * 「已处理 N 分」 and only anchor to nodes on the active branch). Remaps `grouping.index` to match the shifted rows.
 */
export function insertHaaTasks(grouping: Grouping, nodes: readonly Node[], tasks: readonly HaaTask[]): Grouping {
  if (tasks.length === 0) return grouping;

  const beforeFirst: Array<{ order: number; task: HaaTask }> = [];
  const afterRow = new Map<number, Array<{ order: number; task: HaaTask }>>();

  for (let order = 0; order < tasks.length; order++) {
    const task = tasks[order]!;
    const idx = anchorRowIndex(grouping, nodes, task.endedMs);
    if (idx === null) {
      beforeFirst.push({ order, task });
    } else {
      const bucket = afterRow.get(idx);
      if (bucket) bucket.push({ order, task });
      else afterRow.set(idx, [{ order, task }]);
    }
  }

  const out: Row[] = [];
  const remap: number[] = new Array(grouping.rows.length);

  for (const { order, task } of beforeFirst) {
    out.push({ kind: "haaTask", key: taskRowKey(order, task), turn: -1, task });
  }
  for (let i = 0; i < grouping.rows.length; i++) {
    const row = grouping.rows[i]!;
    remap[i] = out.length;
    out.push(row);
    const attached = afterRow.get(i);
    if (attached) {
      for (const { order, task } of attached) {
        out.push({ kind: "haaTask", key: taskRowKey(order, task), turn: row.turn, task });
      }
    }
  }

  const index = new Map([...grouping.index].map(([k, v]) => [k, remap[v] ?? v]));
  return { ...grouping, rows: out, index };
}
