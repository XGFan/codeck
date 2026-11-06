// One connection's state, folded from ccrpc's events. Pure: `apply` returns a new state.
// Content is not kept here: CoDeck shows it from its own index and live following.

import type {
  ChoiceEvent,
  DialogEvent,
  FeedbackDraft,
  InMsg,
  PermissionRequestEvent,
  ProgressEvent,
  QuestionEvent,
  RewindOption,
  RewindRequest,
  StateName,
  Task,
  TurnEndEvent,
  UnrecognizedEvent,
  Waiting,
} from "./protocol";

/** Something waiting for an answer, as a card. */
export type Request =
  | PermissionRequestEvent
  | QuestionEvent
  | ChoiceEvent
  | RewindRequest
  | (DialogEvent & { request_id: string })
  | (UnrecognizedEvent & { request_id: string });

/** The conversation went back to before a prompt: shown in the transcript until the next prompt. */
export interface Rewound {
  uuid: string;
  /** The code was restored too. */
  withCode: boolean;
  /** The input box had a draft, so the prompt was not put into it. */
  keptDraft: boolean;
}

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
  model: string | null;
  effort: string | null;
  contextTokens: number | null;
  requests: Request[];
  progress: ProgressEvent | null;
  /** What waits in the TUI's own queue (sent when the current tool call ends, or at once with `send_now`). */
  queue: string[];
  tasks: Task[];
  /** The last turn's end (duration, usage, cost, error, aborted). */
  lastTurn: TurnEndEvent | null;
  /** ccrpc's process ended (not the Session). */
  procExit: { code: number | null; stderr: string } | null;
  sessionExited: boolean;
  /** A turn ended while this Session was not on screen. */
  unread: boolean;
  /** Short notices (analysis results, takeovers…), newest last. */
  notices: { id: number; text: string }[];
  /** The Session a Resume started this connection for: it stands for that Session until ccrpc names one. */
  resumeOf: string | null;
  /**
   * The Origin Session of a Fork under way on this connection (`ccrpc new --fork`), until its `fork` event: the
   * connection does not stand for that Session.
   */
  forkOf: string | null;
  /** The model drafted product feedback: Claude Code's card above the input box (sent only from the terminal). */
  feedbackDraft: FeedbackDraft | null;
  /** 「选回退点」 (`/rewind` typed): every prompt that can be rewound to shows its 「回退」. */
  picking: boolean;
  /** A `rewind` command under way: the prompt it opens the confirm page for, and whether the Session waited since. */
  rewinding: { uuid: string; waited: boolean } | null;
  /**
   * The conversation went back to before this prompt, as ccrpc keeps it: until the transcript shows the new Branch
   * (checked where it is shown), the Session changes or the connection ends.
   */
  rewound: Rewound | null;
  pending: PendingPrompt | null;
  /**
   * The text of the prompt Claude Code just put back in its input box (Esc before any reply, `turn_end`'s
   * `restored_text`), until the transcript's prompt is found for it (then kept by id) or a new turn starts.
   */
  withdrawnText: string | null;
}

export function initial(
  conn: string,
  holderId: string | null,
  resumeOf: string | null = null,
  forkOf: string | null = null,
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
    tasks: [],
    lastTurn: null,
    procExit: null,
    sessionExited: false,
    unread: false,
    notices: [],
    resumeOf,
    forkOf,
    feedbackDraft: null,
    picking: false,
    rewinding: null,
    rewound: null,
    pending: null,
    withdrawnText: null,
  };
}

let noticeSeq = 0;
function notice(s: ConnState, text: string): ConnState {
  return { ...s, notices: [...s.notices.slice(-4), { id: ++noticeSeq, text }] };
}

/** Main-line requests end when the Session stops waiting; a background Subagent's stays. */
function dropMainRequests(s: ConnState): Request[] {
  return s.requests.filter((r) => "agent_id" in r && r.agent_id);
}

function upsertRequest(s: ConnState, r: Request): ConnState {
  // A rewind's confirm page is off the screen once anything else waits.
  const rest = s.requests.filter((x) => x.request_id !== r.request_id && x.type !== "rewind");
  // A dialog / unrecognized screen replaces any other screen-only card.
  const others =
    r.type === "dialog" || r.type === "unrecognized" || r.type === "choice" || r.type === "rewind"
      ? rest.filter((x) => x.type === "permission_request" || x.type === "question")
      : rest;
  return { ...s, requests: [...others, r] };
}

/**
 * `rewind` stopped at the confirm page: its card. When the Session already stopped waiting (the page was answered or
 * left in the terminal before the answer arrived), nothing would remove it: none.
 */
export function rewindShown(s: ConnState, r: RewindRequest): ConnState {
  if (s.rewinding?.waited && s.state !== "waiting") return { ...s, rewinding: null };
  return { ...upsertRequest(s, r), rewinding: null };
}

/**
 * The rewind's confirm page was answered with `option`: the card goes; with the conversation restored the prompt is
 * marked (the transcript changes only with the next prompt), with only the code restored a notice says so.
 */
export function rewindAnswered(s: ConnState, r: RewindRequest, option: RewindOption, keptDraft: boolean): ConnState {
  const t = { ...s, requests: s.requests.filter((x) => x.request_id !== r.request_id) };
  if (option === "code") return notice(t, "已恢复代码");
  return { ...t, rewound: { uuid: r.uuid, withCode: option === "both", keptDraft } };
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
        waiting: m.waiting ?? null,
        mode: m.mode ?? null,
        model: m.model ?? null,
        effort: m.effort ?? null,
        contextTokens: m.context_tokens ?? null,
        queue: m.queue ?? [],
        feedbackDraft: m.feedback_draft ?? null,
      };
    case "queue":
      return { ...s, queue: m.items };
    case "state": {
      const waiting = m.state === "waiting" ? (m.waiting ?? null) : null;
      const requests = m.state === "waiting" ? s.requests : dropMainRequests(s);
      return {
        ...s,
        state: m.state,
        waiting,
        requests,
        progress: m.state === "busy" ? s.progress : null,
        // Prompts are picked while idle.
        picking: s.picking && m.state === "idle",
        // A new turn: the last prompt is no longer the one taken back.
        withdrawnText: m.state === "busy" ? null : s.withdrawnText,
        rewinding: s.rewinding && m.state === "waiting" ? { ...s.rewinding, waited: true } : s.rewinding,
      };
    }
    case "block":
      // The tool ran (main line or a background Subagent's): its request is over.
      if (m.kind === "tool_result" && s.requests.some((r) => r.request_id === m.tool_use_id))
        return { ...s, requests: s.requests.filter((r) => r.request_id !== m.tool_use_id) };
      return s;
    case "turn_end":
      return {
        ...s,
        progress: null,
        lastTurn: m,
        withdrawnText: m.aborted && m.restored_text ? m.restored_text : null,
      };
    case "permission_request":
    case "question":
    case "choice":
    case "rewind":
      return upsertRequest(s, m);
    case "dialog":
      return upsertRequest(s, { ...m, request_id: `dialog:${m.waiting_for}` });
    case "unrecognized":
      return upsertRequest(s, { ...m, request_id: `unrecognized:${m.context}` });
    case "session_info":
      return { ...s, mode: m.mode, model: m.model, effort: m.effort ?? null, contextTokens: m.context_tokens };
    case "progress":
      return s.state === "busy" ? { ...s, progress: m } : s;
    case "feedback_draft":
      return { ...s, feedbackDraft: m.draft };
    case "follow_up_failed":
      return notice(s, `排队的消息没有发出：${m.error.message}`);
    case "layout_learned":
      return notice(s, `已学会新界面：${m.title}`);
    case "analysis_failed":
      return notice(s, `界面识别失败：${m.error}`);
    case "session_changed":
      // The first sessionId after `ready` (`new`): the same Session, nothing to reset.
      if (m.previous == null) return { ...s, sessionId: m.session_id };
      return {
        ...initial(s.conn, s.holderId),
        ready: true,
        sessionId: m.session_id,
        cwd: s.cwd,
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
