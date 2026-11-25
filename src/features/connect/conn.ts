// One connection's state, folded from ccrpc's events. Pure: `apply` returns a new state.
// Content is not kept here: CoDeck shows it from its own index and live following.

import type {
  FeedbackDraft,
  InMsg,
  ModelInfo,
  ProgressEvent,
  QueueItem,
  RequestEvent,
  RewindOption,
  RewindRequest,
  StartFailedCode,
  StateName,
  Task,
  TurnEndEvent,
  Waiting,
} from "@/ipc/protocol/ccrpc";

/** Something waiting for an answer, as a card. */
export type Request = RequestEvent;

/**
 * The Host's own `/btw` that keeps the prompt cache (`crates/codeck-host/src/keepalive.rs`, same marker): no card,
 * nothing to answer.
 */
export const isKeepAlive = (r: { type: string; question?: unknown }) =>
  r.type === "btw" && typeof r.question === "string" && r.question.trim().startsWith("ping, just say pong");

/** The conversation went back to before a prompt: shown in the transcript until the next prompt. */
export interface Rewound {
  uuid: string;
  /** The code was restored too. */
  withCode: boolean;
  /** The input box had a draft, so the prompt was not put into it. */
  keptDraft: boolean;
}

/**
 * A tool call the main line's code is making or made (Code Mode), from ccrpc's `code_call`: shown in the transcript
 * until the transcript has it (`codeCalls.ts`).
 */
export interface CodeCall {
  toolUseId: string;
  name: string;
  input: unknown;
  /** The `REPL` call whose code made it; null when ccrpc does not know. */
  code: string | null;
  /** It ended: whether it failed, and the tool's answer when ccrpc had it. */
  end: { isError: boolean; response: unknown } | null;
}

/** Code calls kept: the transcript takes over long before this many. */
const MAX_CODE_CALLS = 200;

/** A prompt sent and not in the indexed transcript yet: a placeholder bubble at its end. */
export interface PendingPrompt {
  /** Its transcript entry (ccrpc's answer); null until ccrpc answers. */
  uuid: string | null;
  /** When it was sent (ms), for the log line on its arrival. */
  at: number;
}

export interface ConnState {
  conn: string;
  holderId: string | null;
  sessionId: string | null;
  cwd: string | null;
  claudeVersion: string | null;
  ready: boolean;
  state: StateName;
  waiting: Waiting | null;
  mode: string | null;
  model: ModelInfo | null;
  effort: string | null;
  contextTokens: number | null;
  requests: Request[];
  progress: ProgressEvent | null;
  /** What waits in the TUI's own queue (sent when the current tool call ends, or at once with `send_now`). */
  queue: QueueItem[];
  /** Code Mode: the calls the main line's code made that ccrpc reported, oldest first (not folded by the Host). */
  codeCalls: CodeCall[];
  tasks: Task[];
  /** The last turn's end (duration, usage, cost, error, aborted). */
  lastTurn: TurnEndEvent | null;
  /** ccrpc's process ended (not the Session). After `session_exit` it is expected and not a failure. */
  procExit: { code: number | null; stderr: string } | null;
  /** `start_failed`: claude did not start (`new`, Resume); ccrpc then exits with 1. */
  startFailed: { code: StartFailedCode; message: string } | null;
  /** `fatal`'s message: why ccrpc could not go on; ccrpc then exits with 1. */
  fatal: string | null;
  sessionExited: boolean;
  /** Short notices (analysis results, takeovers…), newest last. */
  notices: { id: number; text: string }[];
  /** The Session a Resume started this connection for: it stands for that Session until ccrpc names one. */
  resumeOf: string | null;
  /**
   * The Origin Session of a Fork under way on this connection (`ccrpc new --fork`), until its `fork` event: the
   * connection does not stand for that Session.
   */
  forkOf: string | null;
  /**
   * 「新建 Session」's directory: until ccrpc names the Session, the connection stands for the page opened for it at
   * once (selected by the connection's id). Gone after a later `session_changed` (`/clear`).
   */
  newIn: string | null;
  /** The model drafted product feedback: Claude Code's card above the input box (sent only from the terminal). */
  feedbackDraft: FeedbackDraft | null;
  /** 「选回退点」 (`/rewind` typed): every prompt that can be rewound to shows its 「回退」. */
  picking: boolean;
  /** A `rewind` command under way: the prompt it opens the confirm page for, until the page's card comes. */
  rewinding: { uuid: string } | null;
  /**
   * The conversation went back to before this prompt, as ccrpc keeps it: until the transcript shows the new Branch
   * (checked where it is shown), the Session changes or the connection ends.
   */
  rewound: Rewound | null;
  pending: PendingPrompt | null;
}

export function initial(
  conn: string,
  holderId: string | null,
  resumeOf: string | null = null,
  forkOf: string | null = null,
  newIn: string | null = null,
): ConnState {
  return {
    conn,
    holderId,
    sessionId: null,
    cwd: null,
    claudeVersion: null,
    ready: false,
    state: "starting",
    waiting: null,
    mode: null,
    model: null,
    effort: null,
    contextTokens: null,
    requests: [],
    progress: null,
    queue: [],
    codeCalls: [],
    tasks: [],
    lastTurn: null,
    procExit: null,
    startFailed: null,
    fatal: null,
    sessionExited: false,
    notices: [],
    resumeOf,
    forkOf,
    newIn,
    feedbackDraft: null,
    picking: false,
    rewinding: null,
    rewound: null,
    pending: null,
  };
}

/** One connection as the Host folded it (`connectSnapshot`; its `fork` is not this window's). */
export type HostConn = Pick<
  ConnState,
  | "conn"
  | "holderId"
  | "sessionId"
  | "cwd"
  | "claudeVersion"
  | "ready"
  | "state"
  | "waiting"
  | "mode"
  | "model"
  | "effort"
  | "contextTokens"
  | "requests"
  | "progress"
  | "queue"
  | "codeCalls"
  | "tasks"
  | "lastTurn"
  | "procExit"
  | "startFailed"
  | "fatal"
  | "sessionExited"
  | "notices"
  | "feedbackDraft"
> & { disconnected: boolean };

/** A connection this window lost track of (Reload), as the Host has it: what is this window's own starts over. */
export function fromHost(h: HostConn): ConnState {
  return {
    ...initial(h.conn, h.holderId),
    sessionId: h.sessionId,
    cwd: h.cwd,
    claudeVersion: h.claudeVersion,
    ready: h.ready,
    state: h.state,
    waiting: h.waiting,
    mode: h.mode,
    model: h.model,
    effort: h.effort,
    contextTokens: h.contextTokens,
    requests: h.requests,
    progress: h.progress,
    queue: h.queue,
    codeCalls: h.codeCalls,
    tasks: h.tasks,
    lastTurn: h.lastTurn,
    procExit: h.procExit,
    startFailed: h.startFailed,
    fatal: h.fatal,
    sessionExited: h.sessionExited,
    // The Host numbers its notices apart from `noticeSeq`: negative, so they never share an id with later ones.
    notices: h.notices.map((n) => ({ ...n, id: -n.id })),
    feedbackDraft: h.feedbackDraft,
  };
}

let noticeSeq = 0;
function notice(s: ConnState, text: string): ConnState {
  return { ...s, notices: [...s.notices.slice(-4), { id: ++noticeSeq, text }] };
}

/** A request event: the card with its id is updated in place, or added. */
function upsertRequest(s: ConnState, r: Request): ConnState {
  const at = s.requests.findIndex((x) => x.request_id === r.request_id);
  const requests = at < 0 ? [...s.requests, r] : s.requests.map((x, i) => (i === at ? r : x));
  return { ...s, requests };
}

/**
 * The rewind's confirm page was answered with `option` (its card goes with its `request_end`): with the conversation
 * restored the prompt is marked (the transcript changes only with the next prompt), with only the code restored a
 * notice says so.
 */
export function rewindAnswered(s: ConnState, r: RewindRequest, option: RewindOption, keptDraft: boolean): ConnState {
  if (option === "code") return notice(s, "已恢复代码");
  return { ...s, rewound: { uuid: r.uuid, withCode: option === "both", keptDraft } };
}

export function apply(s: ConnState, m: InMsg): ConnState {
  switch (m.type) {
    case "ready":
      return {
        ...s,
        ready: true,
        holderId: m.holder_id,
        sessionId: m.session_id,
        cwd: m.cwd,
        claudeVersion: m.claude_version,
        state: m.state,
        waiting: m.waiting,
        mode: m.mode,
        model: m.model,
        effort: m.effort,
        contextTokens: m.context_tokens,
        requests: m.requests.filter((r) => !isKeepAlive(r)),
        queue: m.queue,
        tasks: m.tasks,
        codeCalls: m.code_calls.map((c) => ({
          toolUseId: c.tool_use_id,
          name: c.name,
          input: c.input,
          code: c.code,
          end: null,
        })),
        feedbackDraft: m.feedback_draft,
      };
    case "queue":
      return { ...s, queue: m.items };
    case "tasks":
      return { ...s, tasks: m.tasks };
    case "code_call": {
      const call = { toolUseId: m.tool_use_id, name: m.name, input: m.input, code: m.code, end: null };
      const rest = s.codeCalls.filter((c) => c.toolUseId !== m.tool_use_id);
      return { ...s, codeCalls: [...rest, call].slice(-MAX_CODE_CALLS) };
    }
    case "code_call_end":
      return {
        ...s,
        codeCalls: s.codeCalls.map((c) =>
          c.toolUseId === m.tool_use_id ? { ...c, end: { isError: m.is_error, response: m.response } } : c,
        ),
      };
    case "state": {
      return {
        ...s,
        state: m.state,
        waiting: m.waiting,
        progress: m.state === "busy" ? s.progress : null,
        // Prompts are picked while idle.
        picking: s.picking && m.state === "idle",
      };
    }
    case "turn_end":
      return {
        ...s,
        progress: null,
        lastTurn: m,
        // An aborted turn's code is stopped: what it was doing is never written, so nothing would take these over.
        codeCalls: m.aborted ? [] : s.codeCalls,
      };
    case "rewind":
      // The confirm page the `rewind` under way opened: its card takes over from 「正在准备回退…」.
      return { ...upsertRequest(s, m), rewinding: s.rewinding?.uuid === m.uuid ? null : s.rewinding };
    case "permission_request":
    case "question":
    case "choice":
    case "dialog":
    case "unrecognized":
    case "btw":
      return isKeepAlive(m) ? s : upsertRequest(s, m);
    case "request_end":
      return { ...s, requests: s.requests.filter((r) => r.request_id !== m.request_id) };
    case "session_info":
      return { ...s, mode: m.mode, model: m.model, effort: m.effort, contextTokens: m.context_tokens };
    case "progress":
      return s.state === "busy" ? { ...s, progress: m } : s;
    case "feedback_draft":
      return { ...s, feedbackDraft: m.draft };
    case "layout_learned":
      return notice(s, `已学会新界面：${m.title}`);
    case "analysis_failed":
      return notice(s, `界面识别失败：${m.error}`);
    case "started":
      // The first sessionId after `ready`: the same Session, nothing to reset.
      return { ...s, sessionId: m.session_id, cwd: m.cwd ?? s.cwd };
    case "start_failed":
      return { ...s, startFailed: { code: m.code, message: m.message } };
    case "fatal":
      return { ...s, fatal: m.message };
    case "session_changed":
      // Another Session: its requests, queue and tasks start over (ccrpc sends the queue and tasks again).
      return {
        ...initial(s.conn, s.holderId),
        ready: true,
        sessionId: m.session_id,
        cwd: m.cwd ?? s.cwd,
        claudeVersion: s.claudeVersion,
        state: s.state,
        waiting: s.waiting,
        mode: s.mode,
        model: s.model,
        effort: s.effort,
      };
    case "fork":
      return { ...s, forkOf: null };
    case "session_exit":
      return {
        ...s,
        sessionExited: true,
        state: "exited",
        requests: [],
        codeCalls: [],
        progress: null,
        feedbackDraft: null,
        picking: false,
        rewinding: null,
        rewound: null,
        pending: null,
      };
    default:
      // Responses, and content events (block text, branch, workflow): not state.
      return s;
  }
}
