import { create } from "zustand";
import { useStore } from "@/state/store";
import { useUi } from "@/state/ui";
import { TRUNCATED_MARK, type ToolCall } from "../model";

/** `useUi.expanded` key of a tool call / thinking block / tool group in connection `conn`. */
export const expandKey = (conn: string, key: string) => `${conn}:${key}`;
export const toolKey = (conn: string, toolUseId: string) => expandKey(conn, `tool:${toolUseId}`);

export const useExpanded = (key: string, dflt = false) => useUi((s) => s.expanded[key] ?? dflt);

interface Full {
  text?: string;
  loading?: boolean;
  error?: boolean;
}

/** Full tool results fetched with `get_tool_result`, by `<conn>:<tool_use_id>`. */
const useFull = create<{ full: Record<string, Full> }>(() => ({ full: {} }));

function fetchFull(conn: string, toolUseId: string) {
  const k = `${conn}:${toolUseId}`;
  const set = (f: Full) => useFull.setState((s) => ({ full: { ...s.full, [k]: f } }));
  if (useFull.getState().full[k]?.loading) return;
  set({ loading: true });
  void useStore
    .getState()
    .command(conn, { type: "get_tool_result", tool_use_id: toolUseId })
    .then((r) => {
      const content = r.success ? r.data?.content : undefined;
      set(typeof content === "string" ? { text: content } : { error: true });
    });
}

export interface OutputState {
  text: string;
  /** The inline text is cut (ccrpc 100KB) or a preview of a persisted file: the full one can be fetched. */
  canLoad: boolean;
  loaded: boolean;
  loading: boolean;
  error: boolean;
  totalBytes: number;
  /** Where the full output lives (the persisted file's name). */
  source: string | null;
  /** Even the full text was cut at 100KB. */
  capped: boolean;
  load: () => void;
}

export function useOutput(conn: string, call: ToolCall): OutputState {
  const r = call.result;
  const full = useFull((s) => s.full[`${conn}:${call.id}`]);
  const canLoad = !!r && (r.truncated || r.persisted != null);
  const text = full?.text ?? r?.content ?? "";
  return {
    text,
    canLoad,
    loaded: full?.text != null,
    loading: full?.loading === true,
    error: full?.error === true,
    totalBytes: r?.persisted?.size ?? 0,
    source: r?.persisted ? r.persisted.path.split("/").pop() || r.persisted.path : null,
    capped: full?.text != null && full.text.endsWith(TRUNCATED_MARK),
    load: () => fetchFull(conn, call.id),
  };
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export const str = (v: unknown): string => (typeof v === "string" ? v : "");
export const num = (v: unknown): number | null => (typeof v === "number" ? v : null);
