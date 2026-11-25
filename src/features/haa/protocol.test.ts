import { describe, expect, it } from "vitest";
import { parseHaaWorkers } from "./protocol";

describe("parseHaaWorkers", () => {
  it("parses a valid array of workers with full and minimal fields", () => {
    const raw = JSON.stringify([
      {
        id: "w-1",
        owner: "s-1",
        name: "worker-a",
        state: "running",
        tokens: 12345,
        ctx: 42.6,
        task: "run tests",
        title: "跑单测",
        activity: "vitest run",
        startedAt: 1700000000000,
        pending: "继续吗？",
      },
      {
        owner: "s-2",
        name: "worker-b",
        state: "idle",
      },
    ]);
    expect(parseHaaWorkers(raw)).toEqual([
      {
        id: "w-1",
        owner: "s-1",
        name: "worker-a",
        state: "running",
        tokens: 12345,
        ctx: 42.6,
        task: "run tests",
        title: "跑单测",
        activity: "vitest run",
        startedAt: 1700000000000,
        pending: "继续吗？",
      },
      {
        id: "s-2:worker-b",
        owner: "s-2",
        name: "worker-b",
        state: "idle",
        tokens: 0,
      },
    ]);
  });

  it("accepts all six valid worker states and empty arrays", () => {
    expect(parseHaaWorkers("[]")).toEqual([]);
    const states = ["running", "asking", "idle", "dormant", "taken", "dead"] as const;
    const raw = JSON.stringify(states.map((state, i) => ({ owner: "s", name: `w${i}`, state, tokens: i })));
    const parsed = parseHaaWorkers(raw);
    expect(parsed?.map((w) => w.state)).toEqual(states);
  });

  it("returns null on invalid JSON or non-array values", () => {
    expect(parseHaaWorkers("")).toBeNull();
    expect(parseHaaWorkers("{not json")).toBeNull();
    expect(parseHaaWorkers("{}")).toBeNull();
    expect(parseHaaWorkers("null")).toBeNull();
    expect(parseHaaWorkers("123")).toBeNull();
    expect(parseHaaWorkers('"hello"')).toBeNull();
  });

  it("skips malformed entries individually inside a valid array", () => {
    const raw = JSON.stringify([
      null,
      123,
      "str",
      { owner: "s1", name: "no-state" },
      { owner: "s1", state: "running" },
      { name: "no-owner", state: "running" },
      { owner: "s1", name: "bad-state", state: "error" },
      { owner: "s1", name: "good", state: "asking", tokens: 10 },
    ]);
    expect(parseHaaWorkers(raw)).toEqual([{ id: "s1:good", owner: "s1", name: "good", state: "asking", tokens: 10 }]);
  });

  it("filters negative/invalid numbers and empty optional strings", () => {
    const raw = JSON.stringify([
      {
        id: "",
        owner: "s1",
        name: "w1",
        state: "dead",
        tokens: -5,
        ctx: "bad",
        startedAt: null,
        task: "",
        title: "",
        activity: "",
        pending: "",
      },
    ]);
    expect(parseHaaWorkers(raw)).toEqual([{ id: "s1:w1", owner: "s1", name: "w1", state: "dead", tokens: 0 }]);
  });

  it("parses extended optional fields and drops fields with invalid types without dropping the worker", () => {
    const raw = JSON.stringify([
      {
        id: "w-ext",
        owner: "s1",
        name: "alpha",
        state: "idle",
        agent: "codex",
        model: "codex/gpt-6.2-sol:high",
        cwd: "/repo/worktree",
        tasks: 3,
        cost: 0.42,
        lastActive: 1700000123000,
        tokens: 8000,
      },
      {
        id: "w-bad-ext",
        owner: "s1",
        name: "beta",
        state: "dormant",
        agent: 123,
        model: "",
        cwd: false,
        tasks: "3",
        cost: -0.5,
        lastActive: -10,
        tokens: 100,
      },
    ]);
    expect(parseHaaWorkers(raw)).toEqual([
      {
        id: "w-ext",
        owner: "s1",
        name: "alpha",
        state: "idle",
        agent: "codex",
        model: "codex/gpt-6.2-sol:high",
        cwd: "/repo/worktree",
        tasks: 3,
        cost: 0.42,
        lastActive: 1700000123000,
        tokens: 8000,
      },
      {
        id: "w-bad-ext",
        owner: "s1",
        name: "beta",
        state: "dormant",
        tokens: 100,
      },
    ]);
  });
});
