// Code Mode: the tool calls Claude's code makes reach the transcript only once the code's result is delivered. While
// it runs, ccrpc reports them (`code_call`); here they are put into the transcript as shown, as the blocks they will
// be: after their code, each naming it in `viaCode`. Pure.

import type { AssistantBlock, Node, ToolResult } from "@/ipc/bindings";
import type { CodeCall } from "./conn";

type ToolCallBlock = Extract<AssistantBlock, { kind: "toolCall" }>;

/** What a tool answered, as the text of its result: a string, or a command's stdout and stderr. */
function responseText(response: unknown): string {
  if (typeof response === "string") return response;
  if (!response || typeof response !== "object") return "";
  const r = response as Record<string, unknown>;
  return [r.stdout, r.stderr].filter((x): x is string => typeof x === "string" && x !== "").join("\n");
}

function resultOf(end: NonNullable<CodeCall["end"]>): ToolResult {
  const r = end.response;
  return {
    isError: end.isError,
    timestampMs: null,
    text: responseText(r),
    images: [],
    totalBytes: 0,
    truncated: false,
    persisted: null,
    extraJson: r && typeof r === "object" ? JSON.stringify(r) : null,
  };
}

const blockOf = (c: CodeCall, code: string): ToolCallBlock => ({
  kind: "toolCall",
  toolUseId: c.toolUseId,
  name: c.name,
  inputJson: JSON.stringify(c.input ?? {}),
  inputTruncated: false,
  result: c.end && resultOf(c.end),
  subagentId: null,
  workflowRunId: null,
  notificationNodeId: null,
  code: null,
  viaCode: code,
});

/**
 * `nodes` with the calls of `calls` the transcript does not have yet, each after its code (the `REPL` call it names;
 * failing that the last code still without an outcome) and the calls already listed for it. Left out: a call whose
 * code is not here or already has its outcome (its calls came with it: this one never will), and a question still
 * waiting (answered on its card in the input area; it shows here once answered).
 */
export function withCodeCalls(nodes: Node[], calls: readonly CodeCall[]): Node[] {
  if (calls.length === 0) return nodes;
  const have = new Set<string>();
  /** Code's tool_use id → the node holding it. */
  const codeAt = new Map<string, number>();
  let lastRunning: string | null = null;
  for (const [i, n] of nodes.entries()) {
    if (n.body.kind !== "assistant") continue;
    for (const b of n.body.blocks) {
      if (b.kind !== "toolCall") continue;
      have.add(b.toolUseId);
      if (b.code && b.code.outcome == null) {
        codeAt.set(b.toolUseId, i);
        lastRunning = b.toolUseId;
      }
    }
  }
  /** Node index → code id → the calls to add. */
  const add = new Map<number, Map<string, CodeCall[]>>();
  for (const c of calls) {
    if (have.has(c.toolUseId)) continue;
    if (c.name === "AskUserQuestion" && !c.end) continue;
    const code = c.code != null && codeAt.has(c.code) ? c.code : c.code == null ? lastRunning : null;
    if (code == null) continue;
    const at = codeAt.get(code)!;
    const byCode = add.get(at) ?? new Map<string, CodeCall[]>();
    byCode.set(code, [...(byCode.get(code) ?? []), c]);
    add.set(at, byCode);
  }
  if (add.size === 0) return nodes;
  return nodes.map((n, i) => {
    const byCode = add.get(i);
    if (!byCode || n.body.kind !== "assistant") return n;
    const blocks: AssistantBlock[] = [];
    /** The code whose calls are being passed: its new ones go after the last of them. */
    let open: string | null = null;
    const close = () => {
      if (open != null) blocks.push(...byCode.get(open)!.map((c) => blockOf(c, open!)));
      open = null;
    };
    for (const b of n.body.blocks) {
      if (open != null && !(b.kind === "toolCall" && b.viaCode === open)) close();
      if (b.kind === "toolCall" && b.code && byCode.has(b.toolUseId)) {
        open = b.toolUseId;
        blocks.push({ ...b, code: { ...b.code, callCount: b.code.callCount + byCode.get(b.toolUseId)!.length } });
      } else blocks.push(b);
    }
    close();
    return { ...n, body: { ...n.body, blocks } };
  });
}
