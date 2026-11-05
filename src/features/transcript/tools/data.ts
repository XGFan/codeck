import { create } from "zustand";
import type { ToolCall, TranscriptScope } from "@/ipc/bindings";
import { useToolDetail } from "@/queries/hooks";
import { useUi } from "@/state/ui";

export const toolKey = (nodeId: string, toolUseId: string) => `tool:${nodeId}|${toolUseId}`;

/** Where a call lives; defaults come from the UI store (`ToolCallView` accepts overrides for panels). */
export interface ToolCtx {
  sessionId: string;
  scope: TranscriptScope;
  nodeId: string;
}

export function useToolCtx(nodeId: string, sessionId?: string, scope?: TranscriptScope): ToolCtx | null {
  const sid = useUi((s) => s.sessionId);
  const sc = useUi((s) => s.scope);
  const id = sessionId ?? sid;
  return id ? { sessionId: id, scope: scope ?? sc, nodeId } : null;
}

/** Tool calls whose full output has been requested (by the user or by find). */
const useRequested = create<{ requested: Record<string, true> }>(() => ({ requested: {} }));

/**
 * Asks a tool call to expand and load its full output (`get_tool_detail(Output)`, incl. persisted files).
 * For ⌘F (A14e): call this when a match lies in a tool output whose preview is truncated; the renderer
 * shows the full text once loaded and the react-query cache (`queryKeys.toolDetail`) is shared.
 */
export function requestFullOutput(nodeId: string, toolUseId: string): void {
  const key = toolKey(nodeId, toolUseId);
  useRequested.setState((s) => ({ requested: { ...s.requested, [key]: true } }));
  useUi.getState().setExpanded(key, true);
}

export function parseJson<T = Record<string, unknown>>(text: string | null | undefined): T | null {
  if (!text) return null;
  try {
    return JSON.parse(text) as T;
  } catch {
    return null;
  }
}

/** Parsed tool input; when the stored input is truncated and `enabled`, loads the full one first (A11). */
export function useFullInput(call: ToolCall, ctx: ToolCtx | null, enabled: boolean) {
  const base = parseJson(call.inputJson) ?? {};
  const need = call.inputTruncated && enabled && ctx != null;
  const q = useToolDetail(
    need ? { sessionId: ctx.sessionId, scope: ctx.scope, toolUseId: call.toolUseId, part: "input" } : null,
    need,
  );
  const full = q.data ? parseJson(q.data.text) : null;
  return {
    input: (full ?? base) as Record<string, unknown>,
    /** True while the full input is still being fetched: do not diff / highlight yet. */
    loading: need && !full && !q.isError,
    truncated: call.inputTruncated && !full,
  };
}

export interface OutputState {
  text: string;
  /** True when the preview is cut and the full output can be fetched. */
  canLoad: boolean;
  loaded: boolean;
  loading: boolean;
  error: boolean;
  totalBytes: number;
  /** Human text for where the full output comes from. */
  source: string | null;
  capped: boolean;
  load: () => void;
}

/** Result text plus on-demand full output. `preview` overrides the inline text (e.g. Bash stdout/stderr). */
export function useOutput(call: ToolCall, ctx: ToolCtx | null, preview?: string): OutputState {
  const result = call.result;
  const key = ctx ? toolKey(ctx.nodeId, call.toolUseId) : "";
  const requested = useRequested((s) => !!s.requested[key]);
  const canLoad = !!result && (result.truncated || result.persisted != null) && ctx != null;
  const q = useToolDetail(
    canLoad && requested
      ? { sessionId: ctx.sessionId, scope: ctx.scope, toolUseId: call.toolUseId, part: "output" }
      : null,
    canLoad && requested,
  );
  const persistedName = result?.persisted?.fileName;
  const source = q.data
    ? q.data.source === "persistedFile"
      ? `tool-results/${persistedName ?? ""}`
      : q.data.source === "missing"
        ? "文件缺失"
        : "会话记录"
    : persistedName
      ? `tool-results/${persistedName}`
      : null;
  return {
    text: q.data && q.data.source !== "missing" ? q.data.text : (preview ?? result?.text ?? ""),
    canLoad,
    loaded: !!q.data && q.data.source !== "missing",
    loading: q.isFetching,
    error: q.isError || q.data?.source === "missing",
    totalBytes: q.data?.totalBytes ?? result?.totalBytes ?? 0,
    source,
    capped: q.data?.truncated ?? false,
    load: () => ctx && requestFullOutput(ctx.nodeId, call.toolUseId),
  };
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
