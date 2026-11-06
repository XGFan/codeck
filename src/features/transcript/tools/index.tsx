import type { ToolCall, TranscriptScope } from "@/ipc/bindings";
import {
  BashView,
  CompactView,
  EditView,
  GenericView,
  ReadView,
  SendMessageView,
  TaskView,
  WriteView,
} from "./renderers";
import { parseJson } from "../util";
import { useToolCtx } from "./data";

export { ImageRefs, ImageThumb, PendingText } from "./parts";
export { requestFullOutput } from "./data";

const SUMMARY_KEYS = ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"];

/** One short line describing a call's input: the first well-known string field, else the first string. */
export function summarizeInput(inputJson: string): string {
  const v = parseJson(inputJson);
  if (v && typeof v === "object") {
    for (const k of SUMMARY_KEYS) if (typeof v[k] === "string" && v[k]) return firstLine(v[k]);
    for (const x of Object.values(v)) if (typeof x === "string" && x) return firstLine(x);
  }
  return "";
}

const firstLine = (s: string) => s.split("\n", 1)[0]!.slice(0, 200);

export interface ToolCallViewProps {
  nodeId: string;
  call: ToolCall;
  startMs: number | null;
  /** Overrides for views outside the main transcript (e.g. the subagent panel); default to the UI store. */
  sessionId?: string;
  scope?: TranscriptScope;
}

/** Collapsible tool call with a per-tool renderer; expansion state lives in the store (`tool:<nodeId>|<toolUseId>`). */
export function ToolCallView({ nodeId, call, startMs, sessionId, scope }: ToolCallViewProps) {
  const ctx = useToolCtx(nodeId, sessionId, scope);
  const p = { nodeId, call, startMs, ctx };
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
    case "TaskCreate":
    case "TaskUpdate":
    case "TaskList":
      return <TaskView {...p} />;
    case "SendMessage":
      return <SendMessageView {...p} />;
    case "Grep":
    case "Glob":
    case "WebFetch":
    case "WebSearch":
      return <CompactView {...p} />;
    default:
      return <GenericView {...p} summary={summarizeInput(call.inputJson)} />;
  }
}
