// One connection's state, folded from ccrpc's events. Pure: `apply` returns a new state.
// Content is not kept here: CoDeck shows it from its own index and live following.

import type {
  ChoiceEvent,
  DialogEvent,
  InMsg,
  PermissionRequestEvent,
  ProgressEvent,
  QuestionEvent,
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
  | (DialogEvent & { request_id: string })
  | (UnrecognizedEvent & { request_id: string });

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
}

export function initial(conn: string, holderId: string | null, resumeOf: string | null = null): ConnState {
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
  const rest = s.requests.filter((x) => x.request_id !== r.request_id);
  // A dialog / unrecognized screen replaces any other screen-only card.
  const others =
    r.type === "dialog" || r.type === "unrecognized" || r.type === "choice"
      ? rest.filter((x) => x.type === "permission_request" || x.type === "question")
      : rest;
  return { ...s, requests: [...others, r] };
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
      };
    }
    case "block":
      // The tool ran (main line or a background Subagent's): its request is over.
      if (m.kind === "tool_result" && s.requests.some((r) => r.request_id === m.tool_use_id))
        return { ...s, requests: s.requests.filter((r) => r.request_id !== m.tool_use_id) };
      return s;
    case "turn_end":
      return { ...s, progress: null, lastTurn: m };
    case "permission_request":
    case "question":
    case "choice":
      return upsertRequest(s, m);
    case "dialog":
      return upsertRequest(s, { ...m, request_id: `dialog:${m.waiting_for}` });
    case "unrecognized":
      return upsertRequest(s, { ...m, request_id: `unrecognized:${m.context}` });
    case "session_info":
      return { ...s, mode: m.mode, model: m.model, effort: m.effort ?? null, contextTokens: m.context_tokens };
    case "progress":
      return s.state === "busy" ? { ...s, progress: m } : s;
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
    case "session_exit":
      return { ...s, sessionExited: true, state: "exited", requests: [], progress: null };
    case "taken_over":
      return notice(s, "Session 已被接手，ccrpc 转为只读");
    case "reclaimed":
      return notice(s, "已取回 Session");
    default:
      // Responses, and content events (block text, branch, workflow): not state.
      return s;
  }
}
