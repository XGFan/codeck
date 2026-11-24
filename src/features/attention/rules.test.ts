import { describe, expect, it } from "vitest";
import { due, markNotified, message, step, type Tracks } from "./rules";

const MIN = 60_000;
const DELAYS = { waiting: 3 * MIN, done: 3 * MIN };

describe("step", () => {
  it("marks a turn that ended unseen as done, and clears it once looked at", () => {
    let t = step({}, { a: "busy" }, 0, null);
    t = step(t, { a: "idle" }, 10, "b");
    expect(t.a).toMatchObject({ status: "idle", since: 10, done: true, seen: false });
    t = step(t, { a: "idle" }, 20, "a");
    expect(t.a).toMatchObject({ done: false, seen: true });
  });

  it("does not mark a turn that ended while being looked at, nor idle that was not a turn", () => {
    const seen = step(step({}, { a: "busy" }, 0, null), { a: "idle" }, 10, "a");
    expect(seen.a.done).toBe(false);
    const answered = step(step({}, { a: "waiting" }, 0, null), { a: "idle" }, 10, null);
    expect(answered.a.done).toBe(false);
    expect(step({}, { a: "idle" }, 0, null).a.done).toBe(false);
  });

  it("clears done when it works again and drops Sessions no longer Live", () => {
    let t = step(step({}, { a: "busy", b: "busy" }, 0, null), { a: "idle", b: "idle" }, 10, null);
    t = step(t, { a: "busy" }, 20, null);
    expect(t.a.done).toBe(false);
    expect(t.b).toBeUndefined();
  });

  it("returns the same tracks when nothing changed", () => {
    const t = step({}, { a: "busy" }, 0, null);
    expect(step(t, { a: "busy" }, 50, null)).toBe(t);
  });
});

describe("due", () => {
  const waiting: Tracks = step({}, { w: "waiting" }, 0, null);

  it("is due after the delay, once", () => {
    expect(due(waiting, 3 * MIN - 1, DELAYS)).toEqual([]);
    expect(due(waiting, 3 * MIN, DELAYS)).toEqual([{ id: "w", kind: "waiting", since: 0 }]);
    expect(due(markNotified(waiting, ["w"]), 4 * MIN, DELAYS)).toEqual([]);
  });

  it("is not due when looked at meanwhile, or turned off", () => {
    expect(due(step(waiting, { w: "waiting" }, MIN, "w"), 5 * MIN, DELAYS)).toEqual([]);
    expect(due(waiting, 5 * MIN, { ...DELAYS, waiting: null })).toEqual([]);
  });

  it("counts done from the end of the turn, and starts over on a new status", () => {
    let t = step(step({}, { d: "busy" }, 0, null), { d: "idle" }, MIN, null);
    expect(due(t, 4 * MIN - 1, DELAYS)).toEqual([]);
    expect(due(t, 4 * MIN, DELAYS)).toEqual([{ id: "d", kind: "done", since: MIN }]);
    t = step(t, { d: "waiting" }, 2 * MIN, null);
    expect(due(t, 4 * MIN, DELAYS)).toEqual([]);
  });

  it("sends at once with a zero delay", () => {
    expect(due(waiting, 0, { waiting: 0, done: 0 })).toHaveLength(1);
  });
});

describe("message", () => {
  it("names one Session with how long it waited and its Project", () => {
    const m = message([{ id: "a", kind: "waiting", since: 0, title: "修登录", project: "codeck" }], 3 * MIN + 5000);
    expect(m).toEqual({ title: "修登录", body: "等待作答 3 分钟 · codeck", sessionId: "a" });
    expect(message([{ id: "a", kind: "done", since: 0, title: "修登录", project: null }], 0).body).toBe("已完成");
  });

  it("counts several and opens the first waiting one", () => {
    const m = message(
      [
        { id: "d", kind: "done", since: 0, title: "x", project: null },
        { id: "w", kind: "waiting", since: 5, title: "y", project: null },
        { id: "w2", kind: "waiting", since: 6, title: "z", project: null },
      ],
      MIN,
    );
    expect(m).toEqual({ title: "3 个 Session 需要处理", body: "2 个等待作答，1 个已完成", sessionId: "w" });
  });
});
