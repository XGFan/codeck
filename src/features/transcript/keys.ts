// Expansion keys in `useUi.expanded`: what is open in a transcript. Every key is built here.

/** A tool call's details. */
export const toolKey = (nodeId: string, toolUseId: string) => `tool:${nodeId}|${toolUseId}`;
/** A tool-call group, named after its first call. */
export const groupKey = (nodeId: string, toolUseId: string) => `group:${nodeId}|${toolUseId}`;
/** A compact summary, by its CompactSummary node. */
export const compactKey = (summaryNodeId: string) => `compact:${summaryNodeId}`;
/** A thinking block. */
export const thinkingKey = (nodeId: string) => `thinking:${nodeId}`;
/** The collapsed Fork-inherited prefix. */
export const INHERITED_KEY = "inherited";
/** A turn's folded process, by its prompt node (`fold.ts`). */
export const foldKey = (promptNodeId: string) => `fold:${promptNodeId}`;
export const isFoldKey = (key: string) => key.startsWith("fold:");
