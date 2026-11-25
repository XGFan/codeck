import type { AssistantBlock, ToolCall } from "@/ipc/bindings";
import type { ToolCallBlock } from "./grouping";

/**
 * Code Mode (`REPL`) rules for transcript grouping, counting and block layout. Claude's code block is a `ToolCall`
 * carrying `code` (`callCount`, `outcome`), followed in the same assistant message by sibling `ToolCall` blocks
 * carrying `viaCode` pointing back to its `toolUseId`. Keeping all reads of those two fields here isolates an
 * experimental Claude Code feature from the rest of the transcript pipeline.
 */

/** The calls a call stands for: itself; Claude's code (Code Mode's `REPL`) none, the calls it made have rows of their own. */
export const callsOf = (c: ToolCall): ToolCall[] => (c.code ? [] : [c]);

/** Whether a call is a Code Mode run whose execution ended in an error. */
export const codeFailed = (c: ToolCall): boolean => c.code?.outcome?.isError === true;

/** Number of Code Mode runs that made no tool calls (summarized on their own as 「执行 N 段代码」). */
export const standaloneCodeRuns = (calls: ToolCall[]): number => calls.filter((c) => c.code?.callCount === 0).length;

/**
 * The blocks of an assistant message that are shown, `i` each one's index in the message. Claude's code (Code Mode's
 * `REPL`) comes with the calls it made right after it (`viaCode`), each a call of its own (its question and Subagent
 * cards, its place in a tool group). Code that only asked questions (and did not throw) is left out: its question
 * cards carry it (`code`).
 */
export function shownBlocks(blocks: AssistantBlock[]): Array<{ b: AssistantBlock; i: number; code?: ToolCallBlock }> {
  const calls = blocks.filter((b): b is ToolCallBlock => b.kind === "toolCall");
  const askers = new Map<string, ToolCallBlock>();
  for (const c of calls) {
    if (!c.code || codeFailed(c)) continue;
    const made = calls.filter((m) => m.viaCode === c.toolUseId);
    if (made.length > 0 && made.every((m) => m.name === "AskUserQuestion")) askers.set(c.toolUseId, c);
  }
  return blocks.flatMap((b, i): Array<{ b: AssistantBlock; i: number; code?: ToolCallBlock }> => {
    if (b.kind !== "toolCall") return [{ b, i }];
    if (askers.has(b.toolUseId)) return [];
    return [{ b, i, code: b.viaCode ? askers.get(b.viaCode) : undefined }];
  });
}
