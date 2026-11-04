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
  contextTokens: number | null;
  requests: Request[];
  progress: ProgressEvent | null;
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
}

export function initial(conn: string, holderId: string | null): ConnState {
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
    contextTokens: null,
    requests: [],
    progress: null,
    tasks: [],
    lastTurn: null,
    procExit: null,
    sessionExited: false,
    unread: false,
    notices: [],
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
        contextTokens: m.context_tokens ?? null,
      };
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
      return { ...s, mode: m.mode, model: m.model, contextTokens: m.context_tokens };
    case "progress":
      return s.state === "busy" ? { ...s, progress: m } : s;
    case "follow_up_failed":
      return notice(s, `排队的消息没有发出：${m.error.message}`);
    case "layout_learned":
      return notice(s, `已学会新界面：${m.title}`);
    case "analysis_failed":
      return notice(s, `界面识别失败：${m.error}`);
    case "session_changed":
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
