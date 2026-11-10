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
  /** What the option looks like (an ASCII mockup, code); Claude Code shows it beside the options. */
  preview?: string;
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

/** The `/btw` panel is open: answering (`text` null) or answered. */
export interface BtwEvent {
  type: "btw";
  request_id: string;
  question: string;
  answering: boolean;
  text: string | null;
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
  /** Aborted before any reply: Claude Code put the prompt (this text) back in its input box. */
  restored_text?: string | null;
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
  warnings: string[];
  /** What waits in the TUI's own queue (older ccrpc: absent). */
  queue?: string[];
  /** A feedback draft card already on screen at connect (older ccrpc: absent). */
  feedback_draft?: FeedbackDraft | null;
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
  kind: "agent" | "shell" | "monitor" | "workflow";
  tool_use_id: string | null;
  description: string | null;
  command?: string | null;
  status: string;
  /** A Teammate between rounds: `status` stays `running`, it is not working. */
  idle?: boolean;
}

/** The draft behind Claude Code's 「Product feedback drafted」 card: the model's `SendFeedback` call. */
export interface FeedbackDraft {
  title: string;
  /** The `SendFeedback` call (null when the transcript has none for the card). */
  tool_use_id: string | null;
  input: { title?: string; details?: string; type?: string } | null;
}

/** What a rewind restores: `both` (code and conversation), `conversation`, `code`. */
export type RewindOption = "both" | "conversation" | "code";

/**
 * `rewind`'s answer (and `get_state.request` while it waits): the TUI stopped at the confirm page of going back to
 * before a Main Line prompt, answered with `answer_rewind`.
 */
export interface RewindRequest {
  type: "rewind";
  request_id: string;
  /** The prompt's transcript uuid. */
  uuid: string;
  /** The prompt's full text. */
  text: string;
  /** The page's sentence on the code: `The code will be restored -1 in w.txt.` / `The code will be unchanged.` */
  code: string | null;
  /** The `⚠` warning when code would be restored. */
  warning: string | null;
  /** The restores the page offers (only `conversation` without code changes). */
  options: { option: RewindOption; label: string }[];
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
  | BtwEvent
  | { type: "btw_closed" }
  | { type: "follow_up_failed"; text: string; error: { code: string; message: string } }
  /** The TUI's own input queue changed: steers and what the person typed while busy, oldest first. */
  | { type: "queue"; items: string[] }
  | { type: "layout_learned"; layout: string; kind: string; title: string; path: string }
  | { type: "analysis_failed"; error: string; title?: string }
  | ({ type: "session_info" } & SessionInfo)
  | ProgressEvent
  /** The feedback draft card above the TUI's input box appeared (a draft) or went away (null). */
  | { type: "feedback_draft"; draft: FeedbackDraft | null }
  | { type: "session_changed"; session_id: string; previous: string | null }
  /** A Fork's rewind stopped at the confirm page with the `⚠` warning (`new --fork-restore both`): answered as `rewind`'s. */
  | RewindRequest
  | ForkEvent
  | { type: "session_exit" };

/** `ccrpc new --fork` is done (ADR-0009): the Fork's Session, or why it failed (ccrpc then closes the Pane and exits). */
export type ForkEvent =
  | {
      type: "fork";
      session_id: string;
      origin: string;
      /** The prompt put back into the input box (`! command` for a shell command); null without `--fork-at`. */
      text: string | null;
      title: string | null;
      title_error: string | null;
      error?: undefined;
    }
  | { type: "fork"; error: { code: string; message: string } };

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
