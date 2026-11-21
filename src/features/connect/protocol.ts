// ccrpc JSONL protocol (crates/ccrpc/docs/spec.md §5, docs/protocol.schema.json): the subset Connect uses.
// Content (blocks, get_messages, branch, workflow) comes from CoDeck's own index, so only the
// events that carry state and control are typed here.

/** The protocol version this app speaks: a `ready` with another one is said on the connection. */
export const PROTOCOL = 6;

export type StateName = "starting" | "idle" | "busy" | "waiting" | "exited";
export type WaitKind = "permission" | "question" | "dialog";
export interface Waiting {
  kind: WaitKind;
  waiting_for: string;
}

export type Meaning = "allow" | "allow_always" | "deny" | "other";
export type LayoutSource = "builtin" | "learned";

export interface PermissionOption {
  /** The option's number on the dialog, as a string: unique within the request. */
  id: string;
  label: string;
  meaning: Meaning;
}
export interface PermissionRequestEvent {
  type: "permission_request";
  request_id: string;
  /** The tool call it is for: its row in the transcript. */
  tool_use_id: string;
  tool_name: string;
  input: unknown;
  agent_id: string | null;
  options: PermissionOption[] | null;
  layout_source: LayoutSource | null;
  /** For a call Claude's code made (Code Mode): the transcript has no such call until the code's result is delivered. */
  from_code: boolean;
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
  /** The AskUserQuestion call it is for: its row in the transcript. */
  tool_use_id: string;
  questions: AskQuestion[];
  agent_id: string | null;
  /** Asked from Claude's code (Code Mode's `tools.AskUserQuestion`): see `PermissionRequestEvent.from_code`. */
  from_code: boolean;
}

/**
 * Code Mode: the main line's code (a `REPL` call) made a tool call. Claude Code writes such calls to the transcript
 * only once the code's result is delivered, so until then this is all that says the call is running.
 */
export interface CodeCallEvent {
  type: "code_call";
  tool_use_id: string;
  name: string;
  input: unknown;
  /** The `REPL` call whose code made it (the latest still running); null when ccrpc does not know. */
  code: string | null;
}
/** A call `code_call` reported and not ended yet, as `ready` lists it. */
export type OpenCodeCall = Omit<CodeCallEvent, "type">;
/** A call `code_call` reported ended; `response` is the tool's answer as the plugin's hook had it (null: not said). */
export interface CodeCallEndEvent {
  type: "code_call_end";
  tool_use_id: string;
  is_error: boolean;
  response: unknown;
}
export interface ChoiceOption {
  /** The option's number on the dialog, as a string: unique within the request (`answer_choice`'s `option_id`). */
  id: string;
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
  request_id: string;
  waiting_for: string;
  lines: string[];
  /** The analyzer is still looking at the screen: the same `request_id` comes again with false when it learned nothing. */
  analyzing: boolean;
}
export interface UnrecognizedEvent {
  type: "unrecognized";
  request_id: string;
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

/** What waits for an answer, as ccrpc reports it: one card each, by `request_id`, until its `request_end`. */
export type RequestEvent =
  PermissionRequestEvent | QuestionEvent | ChoiceEvent | RewindRequest | DialogEvent | UnrecognizedEvent | BtwEvent;

/** A request ccrpc reported is over (answered here or in the terminal, its tool ran, its screen went…). */
export interface RequestEndEvent {
  type: "request_end";
  request_id: string;
}

/** One item of the TUI's own input queue. */
export interface QueueItem {
  /** Given when it was queued (`q1`…): unique on this connection, the same while the item stays in the queue. */
  id: string;
  /** `typed`: typed or steered (`[Image #n]` where an image is); `session`: from another Session; `agent`: from a Subagent or Teammate of this one. */
  kind: "typed" | "session" | "agent";
  /** Who sent it (`session` / `agent`); null for what was typed. */
  from: string | null;
  /** What was typed, or the message's body. */
  text: string;
  /** `edit_queue` (↑) takes it back into the input box: what was typed, not a message. */
  editable: boolean;
  /**
   * The paths of the images sent with it, in `[Image #n]` order (`[]` when the text has none); null when ccrpc does not
   * know them (typed in the terminal, queued before this connection).
   */
  images: string[] | null;
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
  restored_text: string | null;
  /** The transcript uuid of that prompt (hidden from the conversation); null when `restored_text` is, or ccrpc does not know. */
  restored_uuid: string | null;
  /** The background tasks still running. */
  background_tasks: Task[];
  duration_ms: number | null;
  usage: Usage;
  api_cost: number | null;
}

/** `id`: the last main-line assistant message's model id (null right after `/model`); `name`: what to show. */
export interface ModelInfo {
  id: string | null;
  name: string;
}

export interface SessionInfo {
  mode: string | null;
  model: ModelInfo | null;
  /** low / medium / high / xhigh / max (or auto); null for a model without effort levels. */
  effort: string | null;
  context_tokens: number | null;
}

/** `get_state`'s answer: everything about the Session now (also all of `ready`). */
export interface Snapshot extends SessionInfo {
  state: StateName;
  waiting: Waiting | null;
  /** What waits for an answer, each as its event. */
  requests: RequestEvent[];
  holder_id: string;
  session_id: string | null;
  cwd: string | null;
  size: [number, number];
  title: string | null;
  /** What waits in the TUI's own queue, oldest first. */
  queue: QueueItem[];
  tasks: Task[];
  /** Code Mode: the calls the main line's code made that have not ended. */
  code_calls: OpenCodeCall[];
  /** A feedback draft card on screen. */
  feedback_draft: FeedbackDraft | null;
}

export interface ReadyEvent extends Snapshot {
  type: "ready";
  protocol: number;
  claude_version: string | null;
  warnings: string[];
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
  command: string | null;
  status: string;
  /** A Teammate between rounds: `status` stays `running`, it is not working. */
  idle: boolean;
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

/** What the confirm page says about the code: whether it is restored, and the diff and files when it says them. */
export interface RewindCode {
  restores: boolean;
  /** `-1`, `+3 -12`. */
  delta: string | null;
  /** `w.txt`, `src/a.rs and README.md`. */
  target: string | null;
}

/**
 * The TUI stopped at the confirm page of going back to before a Main Line prompt (an event, and `rewind`'s answer),
 * answered with `answer_rewind`.
 */
export interface RewindRequest {
  type: "rewind";
  request_id: string;
  /** The prompt's transcript uuid. */
  uuid: string;
  /** The prompt's full text. */
  text: string;
  /** The page's sentence on the code, read; null when it was not read or worded otherwise. */
  code: RewindCode | null;
  /** That sentence as the page has it. */
  code_text: string | null;
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
  | { type: "state"; state: StateName; waiting: Waiting | null }
  | TurnEndEvent
  | RequestEvent
  | RequestEndEvent
  /** The background tasks changed: all of them. */
  | { type: "tasks"; tasks: Task[] }
  /** The TUI's own input queue changed: steers and what the person typed while busy, oldest first. */
  | { type: "queue"; items: QueueItem[] }
  | CodeCallEvent
  | CodeCallEndEvent
  | { type: "layout_learned"; layout: string; kind: string; title: string; path: string }
  | { type: "analysis_failed"; error: string; title?: string }
  | ({ type: "session_info" } & SessionInfo)
  | ProgressEvent
  /** The feedback draft card above the TUI's input box appeared (a draft) or went away (null). */
  | { type: "feedback_draft"; draft: FeedbackDraft | null }
  /** `cwd`: where claude runs. */
  | { type: "session_changed"; session_id: string; previous: string; cwd: string | null }
  /** The Session got its id after `ready` (`new`, an attach to claude's start screen, a Fork's new Session). */
  | { type: "started"; session_id: string; cwd: string | null }
  /**
   * `new` failed to start claude (not a Fork): ccrpc closes the Pane and exits with 1. `code`: `untrusted` (the
   * directory trust box was answered no), `exited` (claude exited before the Session had an id), `no_claude` (no claude
   * process in the Pane after 30 seconds).
   */
  | { type: "start_failed"; code: StartFailedCode; message: string }
  /** ccrpc cannot go on: the last line on stdout, then it exits with 1. */
  | { type: "fatal"; message: string }
  | ForkEvent
  | { type: "session_exit" };

export type StartFailedCode = "untrusted" | "exited" | "no_claude";

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
