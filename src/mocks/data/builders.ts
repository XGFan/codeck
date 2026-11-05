import type { AssistantBlock, ImageRef, Node, PromptOrigin, TokenTotals, ToolCall, ToolResult } from "@/ipc/bindings";

export const NOW = Date.now();
export const MIN = 60_000;
export const HOUR = 60 * MIN;
export const DAY = 24 * HOUR;

export const tok = (input: number, output: number, cacheRead = 0, cacheCreation = 0): TokenTotals => ({
  input,
  output,
  cacheRead,
  cacheCreation,
});

export const sumTok = (...t: TokenTotals[]): TokenTotals =>
  t.reduce(
    (a, b) => tok(a.input + b.input, a.output + b.output, a.cacheRead + b.cacheRead, a.cacheCreation + b.cacheCreation),
    tok(0, 0),
  );

/** USD per MTok: input, output, cache read, cache write (5-minute). */
const RATES: Record<string, [number, number, number, number]> = {
  "claude-opus-4-7": [5, 25, 0.5, 6.25],
  "claude-sonnet-5-5": [2, 10, 0.1, 2.5],
  "claude-haiku-4-5": [1, 5, 0.1, 1.25],
};

/** Mock API cost; `null` when the model has no price. */
export const costOf = (model: string | null, t: TokenTotals): number | null => {
  const r = model ? RATES[model] : undefined;
  if (!r) return null;
  return (t.input * r[0] + t.output * r[1] + t.cacheRead * r[2] + t.cacheCreation * r[3]) / 1e6;
};

interface NodeOpts {
  hidden?: boolean;
  inherited?: boolean;
}

export function userNode(
  id: string,
  ts: number | null,
  text: string,
  o: NodeOpts & { origin?: PromptOrigin; images?: ImageRef[] } = {},
): Node {
  return {
    id,
    timestampMs: ts,
    hidden: o.hidden ?? false,
    inherited: o.inherited ?? false,
    body: { kind: "userPrompt", text, images: o.images ?? [], origin: o.origin ?? { kind: "human" } },
  };
}

export function asstNode(
  id: string,
  ts: number | null,
  blocks: AssistantBlock[],
  o: NodeOpts & { model?: string; usage?: TokenTotals; isApiError?: boolean } = {},
): Node {
  return {
    id,
    timestampMs: ts,
    hidden: o.hidden ?? false,
    inherited: o.inherited ?? false,
    body: {
      kind: "assistant",
      messageId: `msg_${id}`,
      model: o.model ?? "claude-sonnet-5-5",
      blocks,
      usage: o.usage ?? tok(1200, 380, 18_000, 900),
      isApiError: o.isApiError ?? false,
    },
  };
}

export function otherNode(id: string, ts: number | null, body: Node["body"], o: NodeOpts = {}): Node {
  return { id, timestampMs: ts, hidden: o.hidden ?? false, inherited: o.inherited ?? false, body };
}

export const text = (t: string): AssistantBlock => ({ kind: "text", text: t });
export const thinking = (t: string): AssistantBlock => ({ kind: "thinking", text: t });

export function toolResult(r: Partial<ToolResult> & { text: string }): ToolResult {
  return {
    isError: false,
    timestampMs: null,
    images: [],
    totalBytes: r.text.length,
    truncated: false,
    persisted: null,
    extraJson: null,
    ...r,
  };
}

export function call(
  toolUseId: string,
  name: string,
  input: unknown,
  result: (Partial<ToolResult> & { text: string }) | null,
  o: Partial<ToolCall> = {},
): AssistantBlock {
  return {
    kind: "toolCall",
    toolUseId,
    name,
    inputJson: JSON.stringify(input),
    inputTruncated: false,
    result: result && toolResult(result),
    subagentId: null,
    workflowRunId: null,
    notificationNodeId: null,
    ...o,
  };
}

/** 1x1 PNG. */
export const TINY_PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
