import { describe, expect, it } from "vitest";
import { apply, initial, rewindAnswered, type ConnState } from "./conn";
import type {
  InMsg,
  PermissionRequestEvent,
  QueueItem,
  ReadyEvent,
  RewindRequest,
  StateName,
  Task,
  TurnEndEvent,
  Waiting,
} from "./protocol";

const fold = (msgs: InMsg[], s: ConnState = initial("c1", "h1")) => msgs.reduce(apply, s);
const st = (state: StateName, waiting: Waiting | null = null): InMsg => ({ type: "state", state, waiting });
const perm = (id: string, agent: string | null = null): PermissionRequestEvent => ({
  type: "permission_request",
  request_id: id,
  tool_use_id: id,
  tool_name: "Bash",
  input: { command: "ls" },
  agent_id: agent,
  options: null,
  layout_source: null,
  from_code: false,
});
const ready = (o: Partial<ReadyEvent> = {}): ReadyEvent => ({
  type: "ready",
  protocol: 6,
  claude_version: "2.1.329",
  warnings: [],
  state: "idle",
  waiting: null,
  requests: [],
  holder_id: "h1",
  session_id: "s1",
  cwd: "/w",
  size: [80, 24],
  mode: "manual",
  model: { id: "m", name: "M" },
  effort: null,
  context_tokens: 5,
  title: null,
  queue: [],
  tasks: [],
  code_calls: [],
  feedback_draft: null,
  ...o,
});
const task = (id: string, status = "running"): Task => ({
  id,
  kind: "shell",
  tool_use_id: null,
  description: id,
  command: null,
  status,
  idle: false,
});
const rewindReq = (id: string, uuid: string): RewindRequest => ({
  type: "rewind",
  request_id: id,
  uuid,
  text: "改一下 README",
  code: null,
  code_text: null,
  warning: null,
  options: [{ option: "conversation", label: "Restore conversation" }],
});
const end: TurnEndEvent = {
  type: "turn_end",
  prompt_id: "p",
  trigger: "prompt",
  last_text: "ok",
  error: null,
  aborted: false,
  restored_text: null,
  restored_uuid: null,
  background_tasks: [],
  duration_ms: 10,
  usage: { input_tokens: 1, output_tokens: 1, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
  api_cost: 0,
};

describe("conn reducer", () => {
  it("keeps the feedback draft card from ready or its event until it is gone or the Session ended", () => {
    const draft = { title: "T", tool_use_id: "toolu_1", input: { title: "T", details: "- d" } };
    const s = fold([ready({ feedback_draft: draft })]);
    expect(s.feedbackDraft).toEqual(draft);
    expect(fold([{ type: "feedback_draft", draft: null }], s).feedbackDraft).toBeNull();
    expect(fold([{ type: "session_exit" }], s).feedbackDraft).toBeNull();
  });

  it("takes what waits, the queue, the tasks and the running code calls from ready", () => {
    const queue: QueueItem[] = [{ id: "q1", kind: "typed", from: null, text: "再跑一次", editable: true, images: [] }];
    const s = fold([
      ready({
        state: "waiting",
        waiting: { kind: "permission", waiting_for: "permission prompt" },
        requests: [perm("t1")],
        queue,
        tasks: [task("b1")],
        code_calls: [{ tool_use_id: "repl_a", name: "Bash", input: { command: "ls" }, code: "toolu_r1" }],
      }),
    ]);
    expect(s).toMatchObject({ ready: true, state: "waiting", requests: [perm("t1")], queue, tasks: [task("b1")] });
    expect(s.codeCalls).toEqual([
      { toolUseId: "repl_a", name: "Bash", input: { command: "ls" }, code: "toolu_r1", end: null },
    ]);
    // The tasks follow their event, all of them each time.
    expect(apply(s, { type: "tasks", tasks: [task("b1", "completed")] }).tasks).toEqual([task("b1", "completed")]);
  });

  it("a dialog being analyzed is updated in place by its second event, and ends with its request_end", () => {
    const dialog = (analyzing: boolean): InMsg => ({
      type: "dialog",
      request_id: "dialog:1",
      waiting_for: "x",
      lines: ["a"],
      analyzing,
    });
    let s = fold([dialog(true)]);
    expect(s.requests).toMatchObject([{ request_id: "dialog:1", analyzing: true }]);
    s = fold([dialog(false)], s);
    expect(s.requests).toMatchObject([{ request_id: "dialog:1", analyzing: false }]);
    expect(fold([{ type: "request_end", request_id: "dialog:1" }], s).requests).toEqual([]);
  });

  it("keeps a request until its request_end, whatever the state; its event again updates it in place", () => {
    const waiting = st("waiting", { kind: "permission", waiting_for: "permission prompt" });
    let s = fold([waiting, perm("t1"), perm("t2", "agent-1")]);
    s = fold([st("busy"), st("idle"), { ...perm("t1"), options: [{ id: "1", label: "Yes", meaning: "allow" }] }], s);
    expect(s.requests.map((r) => r.request_id)).toEqual(["t1", "t2"]);
    expect((s.requests[0] as PermissionRequestEvent).options).toHaveLength(1);
    // Screen cards do not replace one another: each goes with its own end.
    s = fold(
      [
        { type: "dialog", request_id: "dialog:1", waiting_for: "x", lines: ["a"], analyzing: false },
        { type: "unrecognized", request_id: "unrecognized:2", context: "c", lines: ["b"] },
        { type: "request_end", request_id: "t1" },
        { type: "request_end", request_id: "dialog:1" },
        { type: "request_end", request_id: "nope" },
      ],
      s,
    );
    expect(s.requests.map((r) => r.request_id)).toEqual(["t2", "unrecognized:2"]);
    s = fold([{ type: "btw", request_id: "btw", question: "q", answering: false, text: "a" }], s);
    expect(apply(s, { type: "request_end", request_id: "btw" }).requests.map((r) => r.type)).toEqual([
      "permission_request",
      "unrecognized",
    ]);
    // The Session ended: nothing waits any more.
    expect(apply(s, { type: "session_exit" }).requests).toEqual([]);
  });

  it("keeps the last turn's end and shows progress only while busy", () => {
    let s = fold([{ type: "progress", text: "Thinking…", verb: "Thinking", elapsed_s: 1, tokens: null }]);
    expect(s.progress).toBeNull();
    s = fold([st("busy"), { type: "progress", text: "Thinking…", verb: "Thinking", elapsed_s: 1, tokens: null }], s);
    expect(s.progress?.text).toBe("Thinking…");
    s = apply(s, end);
    expect(s.progress).toBeNull();
    expect(s.lastTurn).toBe(end);
  });

  it("starts over on session_changed, keeping what describes the Pane", () => {
    const old = ready({ session_id: "old" });
    const queued: InMsg = {
      type: "queue",
      items: [{ id: "q1", kind: "typed", from: null, text: "q", editable: true, images: null }],
    };
    let s = fold([old, perm("t1"), queued, { type: "tasks", tasks: [task("b1")] }, end]);
    s = apply(s, { type: "session_changed", session_id: "new", previous: "old", cwd: "/w" });
    expect(s.sessionId).toBe("new");
    expect(s.ready).toBe(true);
    expect(s.cwd).toBe("/w");
    expect(s.mode).toBe("manual");
    expect(s.requests).toEqual([]);
    expect(s.queue).toEqual([]);
    expect(s.tasks).toEqual([]);
    expect(s.lastTurn).toBeNull();

    // Every `session_changed` is another Session, whatever it follows.
    const again = fold([ready({ session_id: "a" }), perm("t1")]);
    expect(apply(again, { type: "session_changed", session_id: "b", previous: "a", cwd: null }).requests).toEqual([]);
  });

  it("`started` names the Session after ready (new) without resetting anything", () => {
    let n = fold([ready({ session_id: null, cwd: null }), perm("t1")]);
    n = apply(n, { type: "started", session_id: "first", cwd: "/n" });
    expect(n.sessionId).toBe("first");
    expect(n.cwd).toBe("/n");
    expect(n.requests.map((r) => r.request_id)).toEqual(["t1"]);
  });

  it("keeps why claude did not start and why ccrpc could not go on; the model is {id, name}", () => {
    expect(fold([{ type: "start_failed", code: "exited", message: "No conversation found" }]).startFailed).toEqual({
      code: "exited",
      message: "No conversation found",
    });
    expect(fold([{ type: "fatal", message: "插件未启用" }]).fatal).toBe("插件未启用");
    const s = fold([
      ready(),
      { type: "session_info", mode: null, model: { id: null, name: "Sonnet 4.5" }, effort: null, context_tokens: null },
    ]);
    expect(s.model).toEqual({ id: null, name: "Sonnet 4.5" });
  });

  it("the confirm page's card takes over from the rewind under way for it", () => {
    const asking = { ...fold([]), rewinding: { uuid: "u1" } };
    // Another prompt's page (a Fork's own rewind): the rewind under way still is.
    expect(apply(asking, rewindReq("rewind:1", "u0")).rewinding).toEqual({ uuid: "u1" });
    const shown = apply(asking, rewindReq("rewind:2", "u1"));
    expect(shown.rewinding).toBeNull();
    expect(shown.requests).toEqual([rewindReq("rewind:2", "u1")]);
    expect(apply(shown, { type: "request_end", request_id: "rewind:2" }).requests).toEqual([]);
  });

  it("marks the rewound prompt until the Session changes or ends; restoring only the code is a notice", () => {
    const rw = rewindReq("rewind:2", "u2");
    const s = apply(fold([]), rw);
    const both = rewindAnswered(s, rw, "both", false);
    expect(both.rewound).toEqual({ uuid: "u2", withCode: true, keptDraft: false });
    expect(rewindAnswered(s, rw, "conversation", true).rewound).toEqual({
      uuid: "u2",
      withCode: false,
      keptDraft: true,
    });
    // As ccrpc keeps it: through turns (a background task's does not fork; a prompt's fork shows in the transcript),
    // until the Session changes or ends.
    const turn: InMsg[] = [st("busy"), st("idle"), end];
    expect(fold(turn, both).rewound).toEqual(both.rewound);
    expect(apply(both, { type: "session_changed", session_id: "s2", previous: "s1", cwd: "/w" }).rewound).toBeNull();
    expect(apply(both, { type: "session_exit" }).rewound).toBeNull();

    const code = rewindAnswered(s, rw, "code", false);
    expect(code.rewound).toBeNull();
    expect(code.notices.map((n) => n.text)).toEqual(["已恢复代码"]);
  });

  it("leaves picking a prompt to rewind to once the Session is not idle", () => {
    const s = { ...fold([st("idle")]), picking: true };
    expect(apply(s, st("idle")).picking).toBe(true);
    expect(apply(s, st("busy")).picking).toBe(false);
  });

  it("keeps the calls the code made as ccrpc reports them, until the turn is aborted or the Session ends", () => {
    const start = (id: string): InMsg => ({
      type: "code_call",
      tool_use_id: id,
      name: "Bash",
      input: { command: id },
      code: "toolu_r1",
    });
    const s = fold([
      start("repl_a"),
      start("repl_b"),
      { type: "code_call_end", tool_use_id: "repl_a", is_error: true, response: null },
    ]);
    expect(s.codeCalls).toEqual([
      {
        toolUseId: "repl_a",
        name: "Bash",
        input: { command: "repl_a" },
        code: "toolu_r1",
        end: { isError: true, response: null },
      },
      { toolUseId: "repl_b", name: "Bash", input: { command: "repl_b" }, code: "toolu_r1", end: null },
    ]);
    // A turn that ended on its own keeps them (the transcript takes them over); an aborted one's are never written.
    expect(fold([end], s).codeCalls).toHaveLength(2);
    expect(fold([{ ...end, aborted: true }], s).codeCalls).toEqual([]);
    expect(fold([{ type: "session_exit" }], s).codeCalls).toEqual([]);
  });
});
