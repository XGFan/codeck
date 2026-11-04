// ccrpc JSONL protocol (crates/ccrpc/docs/spec.md §5, docs/protocol.schema.json): the subset Connect uses.
// Content (blocks, get_messages, branch, workflow) comes from CoDeck's own index, so only the
// events that carry state and control are typed here.

export type StateName = "starting" | "idle" | "busy" | "waiting" | "exited";
export type WaitKind = "permission" | "question" | "dialog";
export interface Waiting {
  kind: WaitKind;
  waiting_for: string;
}

export type Meaning = "allow" | "allow_always" | "deny" | "other";
export type LayoutSource = "builtin" | "learned";

/** A content block: only a tool result matters here (it ends the request for that tool call). */
export interface BlockEvent {
  type: "block";
  kind: string;
  tool_use_id?: string;
}

export interface PermissionOption {
  label: string;
  meaning: Meaning;
}
export interface PermissionRequestEvent {
  type: "permission_request";
  request_id: string;
  tool_name: string;
  input: unknown;
  agent_id: string | null;
  options: PermissionOption[] | null;
  layout_source: LayoutSource | null;
}
export interface QuestionOption {
  label: string;
  description?: string;
}
export interface AskQuestion {
  question: string;
  header?: string;
  options: QuestionOption[];
  multiSelect?: boolean;
}
export interface QuestionEvent {
  type: "question";
  request_id: string;
  questions: AskQuestion[];
  agent_id: string | null;
}
export interface ChoiceOption {
  label: string;
  meaning: Meaning;
  description: string | null;
}
export interface ChoiceEvent {
  type: "choice";
  request_id: string;
  title: string;
  options: ChoiceOption[];
  tool_use_id: string | null;
  layout: string;
  layout_source: LayoutSource;
}
export interface DialogEvent {
  type: "dialog";
  waiting_for: string;
  lines: string[];
}
export interface UnrecognizedEvent {
  type: "unrecognized";
  context: string;
  lines: string[];
}

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cache_read_input_tokens: number;
  cache_creation_input_tokens: number;
}
export interface TurnEndEvent {
  type: "turn_end";
  prompt_id: string | null;
  trigger: "prompt" | "task_notification";
  last_text: string | null;
  error: { kind: string; message: string } | null;
  aborted: boolean;
  background_tasks: unknown[];
  duration_ms: number | null;
  usage: Usage;
  api_cost: number | null;
}

export interface SessionInfo {
  mode: string | null;
  /** A model id (`claude-opus-4-7`), or a display name right after `/model` (`Sonnet 4.5`). */
  model: string | null;
  /** low / medium / high / xhigh / max (or auto); null for a model without effort levels. */
  effort?: string | null;
  context_tokens: number | null;
}

export interface ReadyEvent extends SessionInfo {
  type: "ready";
  protocol: number;
  holder_id: string;
  session_id: string | null;
  cwd: string | null;
  claude_version: string | null;
  state: StateName;
  waiting?: Waiting;
  size: [number, number];
  taken_over: boolean;
  co_display: boolean;
  warnings: string[];
  /** What waits in the TUI's own queue (older ccrpc: absent). */
  queue?: string[];
}

export interface ProgressEvent {
  type: "progress";
  text: string;
  verb: string;
  elapsed_s: number | null;
  tokens: number | null;
}

export interface Task {
  id: string;
  kind: "agent" | "shell" | "workflow";
  tool_use_id: string | null;
  description: string | null;
  command?: string | null;
  status: string;
}

export interface ResponseMsg {
  type: "response";
  id?: string | number;
  command: string;
  success: boolean;
  data?: Record<string, unknown>;
  error?: { code: string; message: string };
}

export type EventMsg =
  | ReadyEvent
  | { type: "state"; state: StateName; waiting?: Waiting }
  | BlockEvent
  | TurnEndEvent
  | PermissionRequestEvent
  | QuestionEvent
  | ChoiceEvent
  | DialogEvent
  | UnrecognizedEvent
  | { type: "follow_up_failed"; text: string; error: { code: string; message: string } }
  /** The TUI's own input queue changed: steers and what the person typed while busy, oldest first. */
  | { type: "queue"; items: string[] }
  | { type: "taken_over"; client: string | null }
  | { type: "reclaimed" }
  | { type: "layout_learned"; layout: string; kind: string; title: string; path: string }
  | { type: "analysis_failed"; error: string; title?: string }
  | ({ type: "session_info" } & SessionInfo)
  | ProgressEvent
  | { type: "session_changed"; session_id: string; previous: string | null }
  | { type: "session_exit" };

export type InMsg = ResponseMsg | EventMsg;

/** One entry of `ccrpc ls --json`. */
export interface LsEntry {
  holder_id: string;
  session_id: string | null;
  status: string | null;
  cwd: string | null;
  plugin: boolean;
  /** The Pane is hidden on the ToH Mac Client (in its hidden window); null outside clay, missing from a ccrpc too old to say. */
  hidden?: boolean | null;
  claude_pid: number | null;
  version: string | null;
  title: string | null;
}
