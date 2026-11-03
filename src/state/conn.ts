// One connection's state, folded from ccrpc's events. Pure: `apply` returns a new state.

import type {
  Block,
  ChoiceEvent,
  DialogEvent,
  InMsg,
  MessagesPage,
  PermissionRequestEvent,
  ProgressEvent,
  QuestionEvent,
  StateName,
  Task,
  TurnEndEvent,
  UnrecognizedEvent,
  Waiting,
  WorkflowEvent,
} from "@/protocol";

/** Something waiting for an answer, as a card. */
export type Request =
  | PermissionRequestEvent
  | QuestionEvent
  | ChoiceEvent
  | (DialogEvent & { request_id: string })
  | (UnrecognizedEvent & { request_id: string });

/** A row of the message stream. */
export type Item = { kind: "block"; key: string; block: Block } | { kind: "turn_end"; key: string; ev: TurnEndEvent };

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
  items: Item[];
  /** Keys (`uuid:index`) of the Main Line blocks in `items`. */
  seen: Set<string>;
  /** Cursor of the oldest loaded block (for `before`), null when nothing is loaded. */
  first: number | null;
  more: boolean;
  /** The first page has arrived. */
  loaded: boolean;
  /** Subagent Run / Workflow Run blocks by the Agent / Workflow call's tool_use id. */
  sub: Record<string, Block[]>;
  subSeen: Set<string>;
  workflows: Record<string, WorkflowEvent>;
  requests: Request[];
  progress: ProgressEvent | null;
  followUps: string[];
  tasks: Task[];
  /** ccrpc's process ended (not the Session). */
  procExit: { code: number | null; stderr: string } | null;
  sessionExited: boolean;
  /** A turn ended while this Session was not on screen. */
  unread: boolean;
  /** Short notices (follow_up failures, analysis results…), newest last. */
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
    items: [],
    seen: new Set(),
    first: null,
    more: false,
    loaded: false,
    sub: {},
    subSeen: new Set(),
    workflows: {},
    requests: [],
    progress: null,
    followUps: [],
    tasks: [],
    procExit: null,
    sessionExited: false,
    unread: false,
    notices: [],
  };
}

export const blockKey = (b: Block) => `${b.uuid}:${b.index}`;
const isSub = (b: Block) => !!b.agent_id || !!b.run_id;

let noticeSeq = 0;
function notice(s: ConnState, text: string): ConnState {
  return { ...s, notices: [...s.notices.slice(-4), { id: ++noticeSeq, text }] };
}

function addSub(s: ConnState, blocks: Block[]): ConnState {
  let sub = s.sub;
  let subSeen = s.subSeen;
  for (const b of blocks) {
    const k = `${b.agent_id ?? b.run_id}:${blockKey(b)}`;
    if (subSeen.has(k) || !b.parent_tool_use_id) continue;
    if (sub === s.sub) {
      sub = { ...s.sub };
      subSeen = new Set(s.subSeen);
    }
    subSeen.add(k);
    sub[b.parent_tool_use_id] = [...(sub[b.parent_tool_use_id] ?? []), b];
  }
  return sub === s.sub ? s : { ...s, sub, subSeen };
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

/** Folds the first (or an older) `get_messages` page in. */
export function applyPage(s: ConnState, page: MessagesPage, older: boolean): ConnState {
  const fresh = page.blocks.filter((b) => !s.seen.has(blockKey(b)));
  const seen = new Set(s.seen);
  for (const b of fresh) seen.add(blockKey(b));
  const rows: Item[] = fresh.map((b) => ({ kind: "block", key: blockKey(b), block: b }));
  let next: ConnState = {
    ...s,
    // An older page goes before everything; the first page goes before the live blocks that
    // arrived while it was being read.
    items: [...rows, ...s.items],
    seen,
    first: page.first ?? s.first,
    more: page.more,
    loaded: true,
  };
  if (!older && page.first == null) next.first = null;
  if (page.subagent_blocks) next = addSub(next, page.subagent_blocks);
  return next;
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
    case "block": {
      // The tool ran (main line or a background Subagent's): its request is over.
      if (m.kind === "tool_result" && s.requests.some((r) => r.request_id === m.tool_use_id))
        s = { ...s, requests: s.requests.filter((r) => r.request_id !== m.tool_use_id) };
      if (isSub(m)) return addSub(s, [m]);
      const key = blockKey(m);
      if (s.seen.has(key)) return s;
      const seen = new Set(s.seen);
      seen.add(key);
      return { ...s, seen, items: [...s.items, { kind: "block", key, block: m }] };
    }
    case "branch": {
      if (m.at_uuid == null) return { ...s, items: [], seen: new Set() };
      let cut = -1;
      for (let i = s.items.length - 1; i >= 0; i--) {
        const it = s.items[i];
        if (it.kind === "block" && it.block.uuid === m.at_uuid) {
          cut = i;
          break;
        }
      }
      // Before what is loaded (an older page, or the first page still coming): start over and reload.
      if (cut < 0) return { ...s, items: [], seen: new Set(), first: null, more: false, loaded: false };
      const items = s.items.slice(0, cut + 1);
      return { ...s, items, seen: new Set(items.filter((i) => i.kind === "block").map((i) => i.key)) };
    }
    case "turn_end":
      return { ...s, progress: null, items: [...s.items, { kind: "turn_end", key: `turn:${s.items.length}:${m.prompt_id}`, ev: m }] };
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
    case "workflow":
      return { ...s, workflows: { ...s.workflows, [m.run_id]: m } };
    case "follow_up_failed":
      return notice(s, `排队的消息没发出去：${m.error.message}`);
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
      return notice(s, "Session 被接手，ccrpc 转为只读");
    case "reclaimed":
      return notice(s, "已取回 Session");
    case "response":
      return s;
    default:
      return s;
  }
}
