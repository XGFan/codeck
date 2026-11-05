import type { AssistantBlock, BranchPoint, InheritedRange, Node, SubagentRun, WorkflowRun } from "@/ipc/bindings";

/**
 * Presentation-only shaping of a Transcript (§3 "Transformation split"): Rust emits normalized nodes, this
 * module turns them into virtual-list rows — turns, tool-call groups, Subagent / Workflow cards, the collapsed
 * Fork prefix, compact dividers and the orphan section.
 */

export type ToolCallBlock = Extract<AssistantBlock, { kind: "toolCall" }>;
type Body<K extends Node["body"]["kind"]> = Extract<Node["body"], { kind: K }>;

/** Role line ("Claude · 10:42") shown on the first row of an assistant run. */
export interface RoleInfo {
  node: Node;
  /** Branch switcher for an assistant head (rendered before the role line). */
  branch: BranchPoint | null;
}

export type GroupItem =
  | { type: "call"; node: Node; call: ToolCallBlock }
  | { type: "thinking"; node: Node; text: string; blockIndex: number };

export interface AgentCardData {
  node: Node;
  call: ToolCallBlock;
  run: SubagentRun;
}

interface RowBase {
  /** Stable virtual-list key derived from node ids. */
  key: string;
  /** Index of the turn (Human/Command prompt) the row belongs to; -1 before the first prompt. */
  turn: number;
  /** Expansion key of the turn's folded process (`fold:<prompt id>`) the row is part of; see `fold.ts`. */
  fold?: string;
}

export type Row = RowBase &
  (
    | { kind: "prompt"; node: Node; body: Body<"userPrompt">; branch: BranchPoint | null }
    | { kind: "taskPrompt"; node: Node; text: string }
    | { kind: "notification"; node: Node; body: Body<"userPrompt"> }
    | {
        kind: "text";
        node: Node;
        text: string;
        role: RoleInfo | null;
        /** The last row of a turn's answer: copy (all of the answer) and time below it. */
        answer?: { text: string; timeMs: number | null };
        /** In a turn: text after its last work is the `answer`, text before or between its work is `process`. */
        layer?: "process" | "answer";
      }
    | { kind: "thinking"; node: Node; text: string; role: RoleInfo | null }
    | { kind: "tool"; node: Node; call: ToolCallBlock; role: RoleInfo | null }
    | {
        kind: "group";
        items: GroupItem[];
        /** Tool names with counts, most frequent first. */
        counts: Array<[string, number]>;
        callCount: number;
        failedCount: number;
        role: RoleInfo | null;
      }
    | ({ kind: "subagent"; role: RoleInfo | null } & AgentCardData)
    | { kind: "parallel"; node: Node; cards: AgentCardData[]; role: RoleInfo | null }
    | { kind: "workflow"; node: Node; call: ToolCallBlock; run: WorkflowRun; role: RoleInfo | null }
    | { kind: "ask"; node: Node; call: ToolCallBlock; role: RoleInfo | null }
    | { kind: "unknownBlock"; node: Node; blockType: string; rawJson: string; role: RoleInfo | null }
    | { kind: "compact"; boundary: Node; summary: Node | null }
    | { kind: "node"; node: Node }
    | {
        kind: "inheritedBanner";
        range: InheritedRange | null;
        count: number;
        expanded: boolean;
        lastTimestampMs: number | null;
      }
    | { kind: "inheritedDivider" }
    | { kind: "orphanHeader"; count: number }
    /** A turn's process folded into one line (`fold.ts`); `key` is its expansion key. */
    | { kind: "fold"; open: boolean; ms: number | null; texts: number; calls: number }
    | { kind: "orphan"; run: SubagentRun }
    | { kind: "finalResult"; run: SubagentRun; text: string }
  );

export interface GroupingInput {
  nodes: Node[];
  branchPoints: BranchPoint[];
  subagents: SubagentRun[];
  workflows: WorkflowRun[];
  inherited: InheritedRange | null;
  orphanSubagentIds: string[];
  /** Hidden nodes are skipped unless set (the backend already filters them unless `include_hidden`). */
  showHidden: boolean;
  /** Fork-inherited prefix expanded (`expanded.inherited`). */
  inheritedExpanded: boolean;
  /** Subagent scope: the first prompt renders as the task card and the run's final text closes the list. */
  agent?: SubagentRun | null;
}

export interface Grouping {
  rows: Row[];
  /** `nodeId` → first row showing it; `nodeId|toolUseId` → row showing that call. Collapsed inherited nodes map to the banner. */
  index: Map<string, number>;
  /** `nodeId|toolUseId` → expansion key of the group holding that call; `thinking:<nodeId>` → group holding that thinking. */
  groupOf: Map<string, string>;
  /** `toolUseId` → node id of the assistant message holding the call. */
  toolNode: Map<string, string>;
  /** Tool calls rendered as Subagent / Workflow cards (they have no `tool:` expansion). */
  cardCalls: Set<string>;
  /** Fork-inherited node ids. */
  inheritedIds: Set<string>;
  /** Fold key of the last turn when it has no answer yet (still running): open unless closed by hand. */
  liveFold: string | null;
  /** CompactSummary node id → its expansion key. */
  compactKeyOf: Map<string, string>;
}

export const groupKey = (nodeId: string, toolUseId: string) => `group:${nodeId}|${toolUseId}`;
export const compactKey = (summaryNodeId: string) => `compact:${summaryNodeId}`;
export const INHERITED_KEY = "inherited";
export const anchorKey = (nodeId: string, toolUseId?: string | null) => (toolUseId ? `${nodeId}|${toolUseId}` : nodeId);
const thinkingKey = (nodeId: string) => `thinking:${nodeId}`;

const AGENT_TOOLS = new Set(["Agent", "Task"]);

// A coordinator message (SendMessage to a Subagent Run) starts the agent's next round of work.
const isTurnStart = (b: Body<"userPrompt">) =>
  b.origin.kind === "human" || b.origin.kind === "command" || b.origin.kind === "coordinator";

export function buildRows(input: GroupingInput): Grouping {
  const { branchPoints, showHidden, inheritedExpanded, agent } = input;
  const nodes = showHidden ? input.nodes : input.nodes.filter((n) => !n.hidden);
  const runs = new Map(input.subagents.map((r) => [r.agentId, r]));
  const workflows = new Map(input.workflows.map((w) => [w.runId, w]));
  const branchByHead = new Map(branchPoints.map((b) => [b.selectedHeadId, b]));

  const inheritedIds = inheritedSet(input.nodes, input.inherited);
  const inheritedCount = input.inherited?.count ?? inheritedIds.size;

  const rows: Row[] = [];
  const index = new Map<string, number>();
  const groupOf = new Map<string, string>();
  const toolNode = new Map<string, string>();
  const cardCalls = new Set<string>();
  const compactKeyOf = new Map<string, string>();

  let turn = -1;
  let inRun = false;
  let pendingRole: RoleInfo | null = null;
  let group: { items: GroupItem[]; calls: Array<{ node: Node; call: ToolCallBlock }> } | null = null;
  let heldThinking: GroupItem[] = [];
  let bannerAt = -1;
  let dividerDone = false;
  let taskPromptDone = !agent;

  const mark = (k: string, i: number) => {
    if (!index.has(k)) index.set(k, i);
  };
  const push = (row: Row, anchors: Array<[string, string?]> = []) => {
    const i = rows.length;
    rows.push(row);
    for (const [nodeId, toolUseId] of anchors) {
      mark(nodeId, i);
      if (toolUseId) mark(anchorKey(nodeId, toolUseId), i);
    }
    return i;
  };
  const takeRole = () => {
    const r = pendingRole;
    pendingRole = null;
    return r;
  };

  const emitThinking = (items: GroupItem[]) => {
    for (const it of items)
      if (it.type === "thinking")
        push(
          {
            kind: "thinking",
            key: `${it.node.id}#${it.blockIndex}`,
            turn,
            node: it.node,
            text: it.text,
            role: takeRole(),
          },
          [[it.node.id]],
        );
  };

  const flushGroup = () => {
    if (group) {
      const g = group;
      group = null;
      if (g.calls.length === 1) {
        const { node, call } = g.calls[0]!;
        push({ kind: "tool", key: `${node.id}|${call.toolUseId}`, turn, node, call, role: takeRole() }, [
          [node.id, call.toolUseId],
        ]);
      } else {
        const first = g.calls[0]!;
        const key = groupKey(first.node.id, first.call.toolUseId);
        const counts = new Map<string, number>();
        for (const { call } of g.calls) counts.set(call.name, (counts.get(call.name) ?? 0) + 1);
        const anchors: Array<[string, string?]> = [];
        for (const it of g.items) {
          if (it.type === "call") {
            anchors.push([it.node.id, it.call.toolUseId]);
            groupOf.set(anchorKey(it.node.id, it.call.toolUseId), key);
          } else {
            anchors.push([it.node.id]);
            groupOf.set(thinkingKey(it.node.id), key);
          }
        }
        push(
          {
            kind: "group",
            key,
            turn,
            items: g.items,
            counts: [...counts.entries()].sort((a, b) => b[1] - a[1]),
            callCount: g.calls.length,
            failedCount: g.calls.filter(({ call }) => call.result?.isError === true).length,
            role: takeRole(),
          },
          anchors,
        );
      }
    }
    emitThinking(heldThinking);
    heldThinking = [];
  };

  const endRun = () => {
    if (!inRun) return;
    flushGroup();
    if (pendingRole) {
      const n = pendingRole.node;
      push({ kind: "text", key: `${n.id}#empty`, turn, node: n, text: "", role: takeRole() }, [[n.id]]);
    }
    inRun = false;
  };

  const assistant = (node: Node, body: Body<"assistant">) => {
    const branch = branchByHead.get(node.id) ?? null;
    if (!inRun || branch || body.isApiError) {
      endRun();
      inRun = true;
      pendingRole = { node, branch };
    }
    const blocks = body.blocks;
    for (let i = 0; i < blocks.length; i++) {
      const b = blocks[i]!;
      if (b.kind === "toolCall") toolNode.set(b.toolUseId, node.id);
      switch (b.kind) {
        case "text":
          if (!b.text.trim()) break;
          flushGroup();
          push({ kind: "text", key: `${node.id}#${i}`, turn, node, text: b.text, role: takeRole() }, [[node.id]]);
          break;
        case "thinking":
          if (!b.text.trim()) break;
          if (group) heldThinking.push({ type: "thinking", node, text: b.text, blockIndex: i });
          else
            push({ kind: "thinking", key: `${node.id}#${i}`, turn, node, text: b.text, role: takeRole() }, [[node.id]]);
          break;
        case "unknown":
          flushGroup();
          push(
            {
              kind: "unknownBlock",
              key: `${node.id}#${i}`,
              turn,
              node,
              blockType: b.blockType,
              rawJson: b.rawJson,
              role: takeRole(),
            },
            [[node.id]],
          );
          break;
        case "toolCall": {
          const run = agentRun(b, runs);
          const wf = b.workflowRunId ? workflows.get(b.workflowRunId) : undefined;
          if (run) {
            // Consecutive Agent calls of one assistant message ran in parallel.
            const cards: AgentCardData[] = [{ node, call: b, run }];
            let j = i + 1;
            for (; j < blocks.length; j++) {
              const nb = blocks[j]!;
              if (nb.kind === "text" && !nb.text.trim()) continue;
              if (nb.kind === "thinking" && !nb.text.trim()) continue;
              if (nb.kind !== "toolCall") break;
              const nr = agentRun(nb, runs);
              if (!nr) break;
              toolNode.set(nb.toolUseId, node.id);
              cards.push({ node, call: nb, run: nr });
            }
            i = j - 1;
            flushGroup();
            for (const c of cards) cardCalls.add(c.call.toolUseId);
            const anchors = cards.map((c): [string, string] => [node.id, c.call.toolUseId]);
            if (cards.length === 1)
              push(
                { kind: "subagent", key: `${node.id}|${b.toolUseId}`, turn, ...cards[0]!, role: takeRole() },
                anchors,
              );
            else
              push(
                { kind: "parallel", key: `${node.id}|${b.toolUseId}|par`, turn, node, cards, role: takeRole() },
                anchors,
              );
          } else if (wf) {
            flushGroup();
            cardCalls.add(b.toolUseId);
            push(
              { kind: "workflow", key: `${node.id}|${b.toolUseId}`, turn, node, call: b, run: wf, role: takeRole() },
              [[node.id, b.toolUseId]],
            );
          } else if (b.name === "AskUserQuestion") {
            // Shown on its own as the question and the user's answer, never folded into a tool group.
            flushGroup();
            cardCalls.add(b.toolUseId);
            push({ kind: "ask", key: `${node.id}|${b.toolUseId}`, turn, node, call: b, role: takeRole() }, [
              [node.id, b.toolUseId],
            ]);
          } else if (group) {
            group.items.push(...heldThinking, { type: "call", node, call: b });
            heldThinking = [];
            group.calls.push({ node, call: b });
          } else {
            group = { items: [{ type: "call", node, call: b }], calls: [{ node, call: b }] };
          }
          break;
        }
      }
    }
  };

  for (let i = 0; i < nodes.length; i++) {
    const node = nodes[i]!;
    const b = node.body;

    // Fork prefix: one banner row (+ the inherited rows when expanded), then the "new content" divider.
    if (inheritedIds.has(node.id)) {
      if (bannerAt < 0) {
        endRun();
        const lastId = input.inherited?.lastInheritedId;
        const last =
          input.nodes.find((n) => n.id === lastId) ?? [...input.nodes].reverse().find((n) => inheritedIds.has(n.id));
        bannerAt = push({
          kind: "inheritedBanner",
          key: INHERITED_KEY,
          turn,
          range: input.inherited,
          count: inheritedCount,
          expanded: inheritedExpanded,
          lastTimestampMs: last?.timestampMs ?? null,
        });
      }
      if (!inheritedExpanded) {
        mark(node.id, bannerAt);
        if (b.kind === "assistant")
          for (const blk of b.blocks) if (blk.kind === "toolCall") toolNode.set(blk.toolUseId, node.id);
        continue;
      }
    } else if (bannerAt >= 0 && !dividerDone) {
      endRun();
      dividerDone = true;
      push({ kind: "inheritedDivider", key: "inherited-divider", turn });
    }

    switch (b.kind) {
      case "userPrompt": {
        endRun();
        if (!taskPromptDone && b.origin.kind === "human") {
          taskPromptDone = true;
          push({ kind: "taskPrompt", key: node.id, turn, node, text: b.text }, [[node.id]]);
          break;
        }
        if (isTurnStart(b)) {
          turn++;
          push({ kind: "prompt", key: node.id, turn, node, body: b, branch: branchByHead.get(node.id) ?? null }, [
            [node.id],
          ]);
        } else if (b.origin.kind === "taskNotification") {
          push({ kind: "notification", key: node.id, turn, node, body: b }, [[node.id]]);
        } else {
          push({ kind: "node", key: node.id, turn, node }, [[node.id]]);
        }
        break;
      }
      case "assistant":
        assistant(node, b);
        break;
      case "compactBoundary": {
        endRun();
        const next = nodes[i + 1];
        const summary =
          next && next.body.kind === "compactSummary" && inheritedIds.has(next.id) === inheritedIds.has(node.id)
            ? next
            : null;
        const anchors: Array<[string, string?]> = [[node.id]];
        if (summary) {
          anchors.push([summary.id]);
          compactKeyOf.set(summary.id, compactKey(summary.id));
          i++;
        }
        push({ kind: "compact", key: node.id, turn, boundary: node, summary }, anchors);
        break;
      }
      case "compactSummary":
        endRun();
        compactKeyOf.set(node.id, compactKey(node.id));
        push({ kind: "node", key: node.id, turn, node }, [[node.id]]);
        break;
      default:
        endRun();
        push({ kind: "node", key: node.id, turn, node }, [[node.id]]);
    }
  }
  endRun();

  if (!agent && input.orphanSubagentIds.length > 0) {
    const orphans = input.orphanSubagentIds.map((id) => runs.get(id) ?? placeholderRun(id));
    push({ kind: "orphanHeader", key: "orphans", turn, count: orphans.length });
    for (const run of orphans) push({ kind: "orphan", key: `orphan:${run.agentId}`, turn, run });
  }
  if (agent?.finalText)
    push({ kind: "finalResult", key: `final:${agent.agentId}`, turn, run: agent, text: agent.finalText });

  const liveFold = markAnswers(rows);
  return { rows, index, groupOf, toolNode, cardCalls, inheritedIds, compactKeyOf, liveFold };
}

// Work in a turn; the text after the last of it is the turn's answer (a system line after the answer is not work).
const WORK = new Set<Row["kind"]>(["thinking", "tool", "group", "subagent", "parallel", "workflow", "unknownBlock"]);

/**
 * Each turn's answer — the text rows after its last bit of work (tool call, thinking, Subagent, Workflow) — gets
 * `answer` on its last row: copy (all of the answer) and its time go under it, as replies carry no role line.
 * Text rows get their `layer`. The rows before the answer, when they hold any work, are the turn's process and
 * get its `fold` key (folded into one line, `fold.ts`). Returns the fold key of a last turn with no answer yet.
 */
function markAnswers(rows: Row[]): string | null {
  let live: string | null = null;
  let i = 0;
  while (i < rows.length) {
    if (rows[i]!.kind !== "prompt") {
      i++;
      continue;
    }
    let end = i + 1;
    while (end < rows.length && rows[end]!.kind !== "prompt" && rows[end]!.kind !== "orphanHeader") end++;
    let lastWork = i;
    for (let j = i + 1; j < end; j++) if (WORK.has(rows[j]!.kind)) lastWork = j;
    const answer: number[] = [];
    for (let j = lastWork + 1; j < end; j++) {
      const r = rows[j]!;
      if (r.kind === "text" && r.text.trim()) answer.push(j);
    }
    const at = answer[answer.length - 1];
    if (at !== undefined) {
      const last = rows[at] as Extract<Row, { kind: "text" }>;
      const text = answer.map((j) => (rows[j] as Extract<Row, { kind: "text" }>).text.trim()).join("\n\n");
      rows[at] = { ...last, answer: { text, timeMs: last.node.timestampMs } };
    }
    for (const j of answer) rows[j] = { ...(rows[j] as Extract<Row, { kind: "text" }>), layer: "answer" };
    const procEnd = answer[0] ?? end;
    const key = lastWork > i ? `fold:${(rows[i] as Extract<Row, { kind: "prompt" }>).node.id}` : undefined;
    for (let j = i + 1; j < procEnd; j++) {
      const r = rows[j]!;
      rows[j] = { ...r, ...(r.kind === "text" ? { layer: "process" as const } : {}), ...(key ? { fold: key } : {}) };
    }
    if (key && at === undefined && end === rows.length) live = key;
    i = end;
  }
  return live;
}

const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit"]);
const SEARCH_TOOLS = new Set(["Grep", "Glob", "WebSearch", "WebFetch", "ToolSearch"]);

/**
 * What a run of tool calls did, in words — 「读取 1 个文件、编辑 2 个文件、运行 1 条命令」 (files edited counted
 * once per path; other tools as 「等 N 次工具调用」) — and what it touched: file names, commands, patterns.
 */
export function describeCalls(calls: ToolCallBlock[]): { text: string; targets: string[] } {
  let read = 0;
  let commands = 0;
  let searches = 0;
  let other = 0;
  const edited = new Set<string>();
  const targets: string[] = [];
  const add = (t: string | null | undefined) => {
    const v = t?.trim();
    if (v && !targets.includes(v)) targets.push(v);
  };
  for (const c of calls) {
    const input = parseInput(c.inputJson);
    const file = str(input.file_path) ?? str(input.notebook_path);
    if (c.name === "Read") {
      read++;
      add(baseName(file));
    } else if (c.name === "Bash") {
      commands++;
      add(str(input.command)?.split("\n", 1)[0]?.slice(0, 40));
    } else if (SEARCH_TOOLS.has(c.name)) {
      searches++;
      add(str(input.pattern) ?? str(input.query) ?? str(input.url));
    } else if (EDIT_TOOLS.has(c.name)) {
      edited.add(file ?? c.toolUseId);
      add(baseName(file));
    } else {
      other++;
      add(c.name);
    }
  }
  const parts = [
    read && `读取 ${read} 个文件`,
    edited.size && `编辑 ${edited.size} 个文件`,
    commands && `运行 ${commands} 条命令`,
    searches && `搜索 ${searches} 次`,
  ].filter(Boolean);
  const text = parts.length
    ? parts.join("、") + (other ? ` 等 ${calls.length} 次工具调用` : "")
    : `${calls.length} 次工具调用`;
  return { text, targets };
}

function parseInput(inputJson: string): Record<string, unknown> {
  try {
    const v: unknown = JSON.parse(inputJson);
    return v && typeof v === "object" ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

const str = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const baseName = (p: string | null) => (p ? (p.split("/").pop() ?? p) : null);

/** Linked Subagent Run of an Agent/Task call (calls without a resolvable run render as plain tool calls). */
function agentRun(b: AssistantBlock, runs: Map<string, SubagentRun>): SubagentRun | undefined {
  if (b.kind !== "toolCall" || !b.subagentId || !AGENT_TOOLS.has(b.name)) return undefined;
  return runs.get(b.subagentId);
}

function inheritedSet(nodes: Node[], range: InheritedRange | null): Set<string> {
  const ids = new Set(nodes.filter((n) => n.inherited).map((n) => n.id));
  if (range) {
    const last = nodes.findIndex((n) => n.id === range.lastInheritedId);
    for (let i = 0; i <= last; i++) ids.add(nodes[i]!.id);
  }
  return ids;
}

function placeholderRun(agentId: string): SubagentRun {
  return {
    agentId,
    agentType: null,
    description: null,
    name: null,
    parentAgentId: null,
    spawnDepth: 0,
    toolUseId: null,
    workflowRunId: null,
    isAsync: false,
    status: null,
    model: null,
    messageCount: 0,
    toolCallCount: 0,
    tokens: { input: 0, output: 0, cacheRead: 0, cacheCreation: 0 },
    startedMs: null,
    endedMs: null,
    promptPreview: null,
    finalText: null,
  };
}

/** Where a node (and optionally one of its tool calls) is shown, and which store expansion keys reveal it. */
export interface Location {
  /** Store `expanded` keys to set to `true` (inherited prefix, group, tool, thinking, compact summary). */
  expand: string[];
}

/**
 * Expansion keys needed to reveal `nodeId` / `toolUseId`. `grouping` must be built with `inheritedExpanded: true`
 * so calls inside the inherited prefix are grouped. Returns `null` when the node is not in the transcript.
 */
export function locate(
  grouping: Grouping,
  nodeId: string,
  toolUseId?: string | null,
  opts: { thinking?: boolean; toolKey?: (nodeId: string, toolUseId: string) => string } = {},
): Location | null {
  if (!grouping.index.has(nodeId)) return null;
  const expand: string[] = [];
  if (grouping.inheritedIds.has(nodeId)) expand.push(INHERITED_KEY);
  // In a folded turn: its fold opens first.
  const row = grouping.rows[rowIndexOf(grouping, nodeId, toolUseId) ?? -1];
  const fold = row?.kind === "fold" ? row.key : row?.fold;
  if (fold) expand.push(fold);
  const compact = grouping.compactKeyOf.get(nodeId);
  if (compact) expand.push(compact);
  if (opts.thinking) {
    const g = grouping.groupOf.get(thinkingKey(nodeId));
    if (g) expand.push(g);
    expand.push(thinkingKey(nodeId));
  }
  if (toolUseId) {
    const g = grouping.groupOf.get(anchorKey(nodeId, toolUseId));
    if (g) expand.push(g);
    if (!grouping.cardCalls.has(toolUseId)) expand.push((opts.toolKey ?? defaultToolKey)(nodeId, toolUseId));
  }
  return { expand };
}

const defaultToolKey = (nodeId: string, toolUseId: string) => `tool:${nodeId}|${toolUseId}`;

/**
 * Group open state: the user's choice, else open when it holds an expanded call (a lone call row
 * that becomes a group when the next call arrives during live follow keeps the user's expansion visible).
 */
export function isGroupOpen(
  row: Extract<Row, { kind: "group" }>,
  expanded: Record<string, boolean>,
  toolKey: (nodeId: string, toolUseId: string) => string = defaultToolKey,
): boolean {
  return (
    expanded[row.key] ??
    row.items.some((it) => it.type === "call" && expanded[toolKey(it.node.id, it.call.toolUseId)] === true)
  );
}

/** Row index for a node / call, preferring the exact call row. */
export function rowIndexOf(grouping: Grouping, nodeId: string, toolUseId?: string | null): number | undefined {
  return (toolUseId ? grouping.index.get(anchorKey(nodeId, toolUseId)) : undefined) ?? grouping.index.get(nodeId);
}
