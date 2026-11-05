import { describe, expect, it } from "vitest";
import { apply, initial, rewindAnswered, rewindShown, type ConnState } from "./conn";
import type { InMsg, PermissionRequestEvent, RewindRequest, TurnEndEvent } from "./protocol";

const fold = (msgs: InMsg[], s: ConnState = initial("c1", "h1")) => msgs.reduce(apply, s);
const perm = (id: string, agent: string | null = null): PermissionRequestEvent => ({
  type: "permission_request",
  request_id: id,
  tool_name: "Bash",
  input: { command: "ls" },
  agent_id: agent,
  options: null,
  layout_source: null,
});
const end: TurnEndEvent = {
  type: "turn_end",
  prompt_id: "p",
  trigger: "prompt",
  last_text: "ok",
  error: null,
  aborted: false,
  background_tasks: [],
  duration_ms: 10,
  usage: { input_tokens: 1, output_tokens: 1, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
  api_cost: 0,
};

describe("conn reducer", () => {
  it("keeps the feedback draft card from ready or its event until it is gone or the Session ended", () => {
    const draft = { title: "T", tool_use_id: "toolu_1", input: { title: "T", details: "- d" } };
    const ready = {
      type: "ready",
      protocol: 2,
      holder_id: "h1",
      session_id: "s1",
      cwd: "/w",
      claude_version: "2.1.318",
      state: "idle",
      size: [80, 24],
      warnings: [],
      mode: null,
      model: null,
      context_tokens: null,
      feedback_draft: draft,
    } satisfies InMsg;
    const s = fold([ready]);
    expect(s.feedbackDraft).toEqual(draft);
    expect(fold([{ type: "feedback_draft", draft: null }], s).feedbackDraft).toBeNull();
    expect(fold([{ type: "session_exit" }], s).feedbackDraft).toBeNull();
    // A ccrpc too old to say: none.
    expect(fold([{ ...ready, feedback_draft: undefined }]).feedbackDraft).toBeNull();
  });

  it("ends main-line requests when waiting ends, a background Subagent's when its tool ran", () => {
    const waiting: InMsg = {
      type: "state",
      state: "waiting",
      waiting: { kind: "permission", waiting_for: "permission prompt" },
    };
    let s = fold([waiting, perm("t1"), perm("t2", "agent-1")]);
    expect(s.requests.map((r) => r.request_id)).toEqual(["t1", "t2"]);
    s = apply(s, { type: "state", state: "busy" });
    expect(s.requests.map((r) => r.request_id)).toEqual(["t2"]);
    // Text blocks change nothing; the Subagent's own tool result ends its request.
    expect(apply(s, { type: "block", kind: "text" })).toBe(s);
    s = apply(s, { type: "block", kind: "tool_result", tool_use_id: "t2" });
    expect(s.requests).toEqual([]);
  });

  it("re-announcing a request replaces it; a new screen card replaces the previous one", () => {
    let s = fold([perm("t1"), { ...perm("t1"), options: [{ label: "Yes", meaning: "allow" }] }]);
    expect(s.requests).toHaveLength(1);
    expect((s.requests[0] as PermissionRequestEvent).options).toHaveLength(1);
    s = fold(
      [
        { type: "dialog", waiting_for: "x", lines: ["a"] },
        { type: "unrecognized", context: "c", lines: ["b"] },
      ],
      s,
    );
    expect(s.requests.map((r) => r.type)).toEqual(["permission_request", "unrecognized"]);
  });

  it("keeps the last turn's end and shows progress only while busy", () => {
    let s = fold([{ type: "progress", text: "Thinking…", verb: "Thinking", elapsed_s: 1, tokens: null }]);
    expect(s.progress).toBeNull();
    s = fold(
      [
        { type: "state", state: "busy" },
        { type: "progress", text: "Thinking…", verb: "Thinking", elapsed_s: 1, tokens: null },
      ],
      s,
    );
    expect(s.progress?.text).toBe("Thinking…");
    s = apply(s, end);
    expect(s.progress).toBeNull();
    expect(s.lastTurn).toBe(end);
  });

  it("starts over on session_changed, keeping what describes the Pane", () => {
    const ready: InMsg = {
      type: "ready",
      protocol: 2,
      holder_id: "h1",
      session_id: "old",
      cwd: "/w",
      claude_version: "2.1",
      state: "idle",
      size: [80, 24],
      mode: "manual",
      model: "m",
      context_tokens: 5,
      warnings: [],
    };
    let s = fold([ready, perm("t1"), end]);
    s = apply(s, { type: "session_changed", session_id: "new", previous: "old" });
    expect(s.sessionId).toBe("new");
    expect(s.ready).toBe(true);
    expect(s.cwd).toBe("/w");
    expect(s.mode).toBe("manual");
    expect(s.requests).toEqual([]);
    expect(s.lastTurn).toBeNull();

    // `new`: `ready` before the plugin checked in; the first sessionId only fills it in.
    let n = fold([{ ...ready, session_id: null }, perm("t1")]);
    n = apply(n, { type: "session_changed", session_id: "first", previous: null });
    expect(n.sessionId).toBe("first");
    expect(n.requests.map((r) => r.request_id)).toEqual(["t1"]);
  });

  it("keeps the rewind card while the confirm page waits, drops it when the waiting ends or another screen comes", () => {
    const waiting: InMsg = { type: "state", state: "waiting", waiting: { kind: "dialog", waiting_for: "dialog open" } };
    const rw: RewindRequest = {
      type: "rewind",
      request_id: "rewind:1",
      uuid: "u1",
      text: "改一下 README",
      code: "The code will be unchanged.",
      warning: null,
      options: [{ option: "conversation", label: "Restore conversation" }],
    };
    const shown = rewindShown({ ...fold([waiting]), rewinding: { uuid: "u1", waited: true } }, rw);
    expect(shown.requests).toEqual([rw]);
    expect(shown.rewinding).toBeNull();
    expect(apply(shown, waiting).requests).toEqual([rw]);
    // Answered or cancelled in the terminal: the Session leaves the waiting, or the picker shows as a dialog.
    expect(apply(shown, { type: "state", state: "idle" }).requests).toEqual([]);
    expect(
      fold([{ type: "dialog", waiting_for: "dialog open", lines: ["Rewind"] }], shown).requests.map((r) => r.type),
    ).toEqual(["dialog"]);
    expect(apply(shown, perm("t1")).requests.map((r) => r.request_id)).toEqual(["t1"]);

    // The answer arrives after the Session waited and stopped again (the page was left in the terminal): no card.
    const asking = { ...fold([]), rewinding: { uuid: "u1", waited: false } };
    const left = fold([waiting, { type: "state", state: "idle" }], asking);
    expect(left.rewinding).toEqual({ uuid: "u1", waited: true });
    expect(rewindShown(left, rw)).toMatchObject({ requests: [], rewinding: null });
    // Not waited yet (the answer came before the state): the card, the waiting follows.
    expect(rewindShown(asking, rw).requests).toEqual([rw]);
  });

  it("marks the rewound prompt until the Session changes or ends; restoring only the code is a notice", () => {
    const rw: RewindRequest = {
      type: "rewind",
      request_id: "rewind:2",
      uuid: "u2",
      text: "t",
      code: "The code will be restored -1 in w.txt.",
      warning: "w",
      options: [],
    };
    const s = rewindShown(fold([]), rw);
    const both = rewindAnswered(s, rw, "both", false);
    expect(both.requests).toEqual([]);
    expect(both.rewound).toEqual({ uuid: "u2", withCode: true, keptDraft: false });
    expect(rewindAnswered(s, rw, "conversation", true).rewound).toEqual({
      uuid: "u2",
      withCode: false,
      keptDraft: true,
    });
    // As ccrpc keeps it: through turns (a background task's does not fork; a prompt's fork shows in the transcript),
    // until the Session changes or ends.
    const turn: InMsg[] = [{ type: "state", state: "busy" }, { type: "state", state: "idle" }, end];
    expect(fold(turn, both).rewound).toEqual(both.rewound);
    expect(apply(both, { type: "session_changed", session_id: "s2", previous: "s1" }).rewound).toBeNull();
    expect(apply(both, { type: "session_exit" }).rewound).toBeNull();

    const code = rewindAnswered(s, rw, "code", false);
    expect(code.rewound).toBeNull();
    expect(code.notices.map((n) => n.text)).toEqual(["已恢复代码"]);
  });

  it("leaves picking a prompt to rewind to once the Session is not idle", () => {
    const s = { ...fold([{ type: "state", state: "idle" }]), picking: true };
    expect(apply(s, { type: "state", state: "idle" }).picking).toBe(true);
    expect(apply(s, { type: "state", state: "busy" }).picking).toBe(false);
  });
});
