import type { ToolCall } from "../model";
import { BashView, CompactView, EditView, GenericView, ReadView, TodoView, WriteView } from "./renderers";

const SUMMARY_KEYS = ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"];

const firstLine = (s: string) => s.split("\n", 1)[0]!.slice(0, 200);

/** One short line describing a call's input: the first well-known string field, else the first string. */
export function summarizeInput(input: Record<string, unknown>): string {
  for (const k of SUMMARY_KEYS) if (typeof input[k] === "string" && input[k]) return firstLine(input[k]);
  if (Array.isArray(input.questions)) {
    const q = (input.questions[0] as { question?: unknown } | undefined)?.question;
    if (typeof q === "string") return firstLine(q);
  }
  for (const x of Object.values(input)) if (typeof x === "string" && x) return firstLine(x);
  return "";
}

/** Collapsible tool call with a per-tool renderer; expansion state lives in `useUi.expanded` (`<conn>:tool:<id>`). */
export function ToolCallView({ conn, call }: { conn: string; call: ToolCall }) {
  const p = { conn, call };
  switch (call.name) {
    case "Bash":
      return <BashView {...p} />;
    case "Edit":
    case "MultiEdit":
      return <EditView {...p} />;
    case "Write":
      return <WriteView {...p} />;
    case "Read":
      return <ReadView {...p} />;
    case "TodoWrite":
      return <TodoView {...p} />;
    case "Grep":
    case "Glob":
    case "WebFetch":
    case "WebSearch":
      return <CompactView {...p} />;
    default:
      return <GenericView {...p} summary={summarizeInput(call.input)} />;
  }
}
