import { describe, expect, it } from "vitest";
import type { ProjectSummary, SessionSummary } from "@/ipc/bindings";
import {
  buildSidebar,
  fitRows,
  flatten,
  groupKey,
  moveEntry,
  recentRoom,
  restKey,
  reveal,
  subKey,
  suggested,
  workerKey,
} from "./groups";

const NOW = Date.UTC(2026, 10, 6, 12);
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * 60 * MIN;

const project = (path: string, lastActiveMs = NOW): ProjectSummary => ({
  id: path,
  path,
  displayName: path.split("/").pop()!,
  missing: false,
  sessionCount: 1,
  lastActiveMs,
});

const session = (
  id: string,
  projectId: string,
  lastPromptMs: number,
  extra: Partial<SessionSummary> = {},
): SessionSummary => ({
  id,
  agent: "claude",
  haaWorker: null,
  projectId,
  title: id,
  titleSource: "ai",
  createdMs: lastPromptMs,
  lastActiveMs: lastPromptMs,
  lastPromptMs,
  messageCount: 1,
  toolCallCount: 0,
  subagentCount: 0,
  tokens: { input: 0, output: 0, cacheRead: 0, cacheCreation: 0 },
  cost: 0,
  unpricedMessages: 0,
  gitBranch: null,
  live: null,
  forkOrigin: null,
  primaryModel: null,
  editedProjectIds: [],
  ...extra,
});

const live = (status: "busy" | "waiting" | "idle") => ({ status, rawStatus: status, waitingFor: null, pid: 1 });

const W = "/w";
const projects = [project("/a"), project("/b"), project(W), project(`${W}/x`), project(`${W}/y`), project(`${W}/old`)];
const ids = (rows: { id: string }[]) => rows.map((r) => r.id);

describe("buildSidebar", () => {
  it("keeps the groups in the user's order and their Sessions by the latest user input, not the latest reply", () => {
    const sb = buildSidebar(
      [
        // Replied to just now, prompted long ago: stays behind.
        session("a-old", "/a", NOW - 3 * DAY, { lastActiveMs: NOW }),
        session("a-new", "/a", NOW - DAY),
        session("b1", "/b", NOW - MIN),
        session("w-root", W, NOW - 5 * DAY),
      ],
      [],
      projects,
      [{ path: "/a" }, { path: "/b" }],
      NOW,
    );
    // /b has the newest Session but stays second.
    expect(sb.groups.map((g) => g.path)).toEqual(["/a", "/b"]);
    expect(ids(sb.groups[0]!.sessions)).toEqual(["a-new", "a-old"]);
    // 「Recent」 has every Session, in a group or not.
    expect(ids(sb.recent)).toEqual(["b1", "a-new", "a-old", "w-root"]);
  });

  it("nesting gathers the Projects under a directory as sub-groups; a deeper added Project keeps its own", () => {
    const sessions = [
      session("root", W, NOW - DAY),
      session("x1", `${W}/x`, NOW - 2 * DAY),
      session("y1", `${W}/y`, NOW - MIN),
      session("old1", `${W}/old`, NOW - 90 * DAY),
    ];
    const nested = buildSidebar(sessions, [], projects, [{ path: W, nest: true }, { path: `${W}/x` }], NOW);
    const w = nested.groups.find((g) => g.path === W)!;
    expect(ids(w.sessions)).toEqual(["root"]);
    expect(w.subs.map((s) => [s.projectId, s.active])).toEqual([
      [`${W}/y`, true],
      [`${W}/old`, false],
    ]);
    expect(w.total).toBe(3);
    expect(w.childProjects).toBe(3);
    expect(ids(nested.groups.find((g) => g.path === `${W}/x`)!.sessions)).toEqual(["x1"]);

    const flat = buildSidebar(sessions, [], projects, [{ path: W }], NOW);
    expect(ids(flat.groups[0]!.sessions)).toEqual(["root"]);
    expect(flat.groups[0]!.total).toBe(1);
  });

  it("lists Live and pending Sessions under 「Live」, not under 「Recent」; a pending one joins the group its cwd is in", () => {
    const sb = buildSidebar(
      [session("a1", "/a", NOW - DAY, { live: live("waiting") }), session("a2", "/a", NOW - 2 * DAY)],
      [{ id: "p1", cwd: "/a/sub", status: "idle" }],
      projects,
      [{ path: "/a" }],
      NOW,
    );
    expect(ids(sb.live)).toEqual(["p1", "a1"]);
    expect(ids(sb.recent)).toEqual(["a2"]);
    expect(ids(sb.groups[0]!.sessions)).toEqual(["p1", "a1", "a2"]);
    expect(sb.groups[0]!.live).toBe("waiting");
  });

  it("folds haa Workers under their owner, excludes them from groups and Recent, routes unlinked non-Live to unlinked, and keeps Live Workers in Live", () => {
    const worker = (
      id: string,
      ownerSessionId: string | null,
      lastPromptMs: number,
      extra: Partial<SessionSummary> = {},
    ) =>
      session(id, "/a", lastPromptMs, {
        agent: "agy",
        haaWorker: { ownerSessionId, name: `name-${id}`, taskTitle: null },
        ...extra,
      });
    const sb = buildSidebar(
      [
        session("owner", "/a", NOW - 3 * DAY),
        worker("w-old", "owner", NOW - 2 * DAY),
        worker("w-new", "owner", NOW - DAY),
        // Live Worker linked to owner: appears in both owner.workers and Live, never in group totals/live.
        worker("w-live-linked", "owner", NOW - MIN, { live: live("waiting") }),
        // Unlinked cases: null owner, missing owner, owner is itself a Worker.
        worker("w-null", null, NOW - 4 * DAY),
        worker("w-missing", "no-such-id", NOW - 2 * HOUR),
        worker("w-of-worker", "w-old", NOW - 5 * DAY),
        // Unlinked + Live: stays in Live, does not enter unlinked.
        worker("w-live-unlinked", null, NOW - 2 * MIN, { live: live("busy") }),
        // Worker in a nested sub-project must not create a sub-group either.
        worker("w-sub", null, NOW - 6 * DAY, { projectId: `${W}/y` }),
      ],
      [],
      projects,
      [{ path: "/a" }, { path: W, nest: true }],
      NOW,
    );

    const groupA = sb.groups[0]!;
    expect(ids(groupA.sessions)).toEqual(["owner"]);
    expect(groupA.total).toBe(1);
    expect(groupA.lastMs).toBe(NOW - 3 * DAY);
    expect(groupA.live).toBeNull();
    expect(sb.groups[1]!.subs).toEqual([]);

    const ownerRow = groupA.sessions[0]!;
    expect(ownerRow.isWorker).toBe(false);
    expect(ownerRow.workerName).toBeNull();
    expect(ids(ownerRow.workers)).toEqual(["w-live-linked", "w-new", "w-old"]);
    expect(ownerRow.workers.map((w) => [w.agent, w.isWorker, w.workerName])).toEqual([
      ["agy", true, "name-w-live-linked"],
      ["agy", true, "name-w-new"],
      ["agy", true, "name-w-old"],
    ]);

    expect(ids(sb.recent)).toEqual(["owner"]);
    expect(ids(sb.live)).toEqual(["w-live-linked", "w-live-unlinked"]);
    expect(ids(sb.unlinked)).toEqual(["w-missing", "w-null", "w-of-worker", "w-sub"]);
  });
});

describe("flatten", () => {
  const sessions = [
    ...Array.from({ length: 7 }, (_, i) => session(`a${i}`, "/a", NOW - i * MIN)),
    session("y1", `${W}/y`, NOW - DAY),
    session("old1", `${W}/old`, NOW - 90 * DAY),
    session("b1", "/b", NOW - 2 * DAY),
  ];
  const sb = buildSidebar(sessions, [], projects, [{ path: "/a" }, { path: W, nest: true }], NOW);

  it("shows 5 rows then 「显示更多」; sub-groups start closed, inactive sub-Projects folded", () => {
    const items = flatten(sb, {}, {});
    expect(items.map((i) => i.kind)).toEqual(["group", ...Array(5).fill("row"), "more", "group", "sub", "subsRest"]);
    expect(items.find((i) => i.kind === "more")).toMatchObject({ rest: 2 });
    expect(items.find((i) => i.kind === "subsRest")).toMatchObject({ count: 1 });
  });

  it("follows the open state and 「显示更多」", () => {
    const items = flatten(sb, { [groupKey("/a")]: false, [subKey(W, `${W}/y`)]: true, [restKey(W)]: true }, {});
    expect(items.filter((i) => i.kind === "row").map((r) => [r.s.id, r.depth])).toEqual([["y1", 2]]);
    expect(items.filter((i) => i.kind === "sub").map((s) => s.sub.projectId)).toEqual([`${W}/y`, `${W}/old`]);
    expect(flatten(sb, {}, { [groupKey("/a")]: 25 }).filter((i) => i.kind === "row")).toHaveLength(7);
  });

  it("lists the group headers alone while one is dragged", () => {
    expect(flatten(sb, {}, {}, true).map((i) => i.kind)).toEqual(["group", "group"]);
  });

  it("inserts worker items right after their owner only when its w: key is open", () => {
    const withWorkers = buildSidebar(
      [
        session("a0", "/a", NOW - MIN),
        session("w1", "/a", NOW - 2 * MIN, {
          agent: "agy",
          haaWorker: { ownerSessionId: "a0", name: "w1", taskTitle: null },
        }),
        session("w2", "/a", NOW, { agent: "agy", haaWorker: { ownerSessionId: "a0", name: "w2", taskTitle: null } }),
        session("y1", `${W}/y`, NOW - DAY),
        session("wy", `${W}/y`, NOW - HOUR, {
          agent: "agy",
          haaWorker: { ownerSessionId: "y1", name: "wy", taskTitle: null },
        }),
      ],
      [],
      projects,
      [{ path: "/a" }, { path: W, nest: true }],
      NOW,
    );
    // Collapsed by default.
    expect(flatten(withWorkers, {}, {}).filter((i) => i.kind === "worker")).toEqual([]);
    // Expanded under both depth 1 (group) and depth 2 (sub-group).
    const openItems = flatten(
      withWorkers,
      { [workerKey("a0")]: true, [subKey(W, `${W}/y`)]: true, [workerKey("y1")]: true },
      {},
    );
    expect(openItems.filter((i) => i.kind === "worker").map((i) => [i.s.id, i.depth])).toEqual([
      ["w2", 1],
      ["w1", 1],
      ["wy", 2],
    ]);
  });
});

describe("moveEntry", () => {
  it("moves to an insertion point counted before the move", () => {
    const abc = ["a", "b", "c"];
    expect(moveEntry(abc, 2, 0)).toEqual(["c", "a", "b"]);
    expect(moveEntry(abc, 0, 3)).toEqual(["b", "c", "a"]);
    expect(moveEntry(abc, 0, 2)).toEqual(["b", "a", "c"]);
    expect(moveEntry(abc, 1, 1)).toEqual(abc);
  });
});

describe("fitRows with the user's Recent rows", () => {
  it("uses the dragged row count while there is room, and still shrinks Recent first in a short column", () => {
    expect(fitRows(900, 2, 1800, true, 10)).toEqual({ live: 2, recent: 10 });
    expect(fitRows(900, 2, 3, true, 10)).toEqual({ live: 2, recent: 3 });
    // 2 Live (34 + 56) + Recent chrome 37 + the Project list's 155 leave 318 px: 11 rows.
    expect(recentRoom(600, 2)).toBe(Math.floor((600 - 90 - 37 - 155) / 28));
    expect(fitRows(600, 2, 1800, true, 30)).toEqual({ live: 2, recent: recentRoom(600, 2) });
    expect(recentRoom(100, 5)).toBe(1);
  });
});

describe("fitRows", () => {
  it("shows up to 5 Live and 6 Recent rows, fewer of each only when there are fewer", () => {
    expect(fitRows(900, 9, 1800, true)).toEqual({ live: 5, recent: 6 });
    expect(fitRows(900, 2, 3, true)).toEqual({ live: 2, recent: 3 });
    expect(fitRows(900, 2, 3, false)).toEqual({ live: 2, recent: 0 });
  });

  it("in a short column shrinks 「Recent」 first, then 「Live」, keeping the Project list's heading and 4 rows", () => {
    // 5 Live (34 + 140) + 6 Recent (37 + 168) + the Project list's 155 = 534.
    expect(fitRows(534, 9, 1800, true)).toEqual({ live: 5, recent: 6 });
    expect(fitRows(534 - 28 * 3, 9, 1800, true)).toEqual({ live: 5, recent: 3 });
    expect(fitRows(534 - 28 * 7, 9, 1800, true)).toEqual({ live: 3, recent: 1 });
    expect(fitRows(100, 9, 1800, true)).toEqual({ live: 1, recent: 1 });
  });
});

describe("reveal", () => {
  const sessions = [
    ...Array.from({ length: 7 }, (_, i) => session(`a${i}`, "/a", NOW - i * MIN)),
    session("old1", `${W}/old`, NOW - 90 * DAY),
  ];
  const sb = buildSidebar(sessions, [], projects, [{ path: "/a" }, { path: W, nest: true }], NOW);

  it("opens the groups on the way and shows enough rows", () => {
    expect(reveal(sb, "a6", {}, {})).toEqual({ open: [], limits: { [groupKey("/a")]: 7 } });
    expect(reveal(sb, "old1", {}, {})).toEqual({
      open: [restKey(W), subKey(W, `${W}/old`)],
      limits: {},
    });
    expect(reveal(sb, "a0", { [groupKey("/a")]: false }, {})).toEqual({ open: [groupKey("/a")], limits: {} });
  });

  it("does nothing for a row already shown or an unknown Session", () => {
    expect(reveal(sb, "a0", {}, {})).toBeNull();
    expect(reveal(sb, "nope", {}, {})).toBeNull();
  });

  it("reveals a linked Worker by opening its owner's groups, w: key, and expanding limits if needed", () => {
    const sbWithWorkers = buildSidebar(
      [
        ...sessions,
        session("w-a6", "/a", NOW, {
          agent: "agy",
          haaWorker: { ownerSessionId: "a6", name: "w-a6", taskTitle: null },
        }),
        session("w-old", `${W}/old`, NOW, {
          agent: "agy",
          haaWorker: { ownerSessionId: "old1", name: "w-old", taskTitle: null },
        }),
        session("w-unlinked", "/a", NOW, {
          agent: "agy",
          haaWorker: { ownerSessionId: null, name: "w-unlinked", taskTitle: null },
        }),
      ],
      [],
      projects,
      [{ path: "/a" }, { path: W, nest: true }],
      NOW,
    );
    expect(reveal(sbWithWorkers, "w-a6", { [groupKey("/a")]: false }, {})).toEqual({
      open: [groupKey("/a"), workerKey("a6")],
      limits: { [groupKey("/a")]: 7 },
    });
    expect(reveal(sbWithWorkers, "w-old", {}, {})).toEqual({
      open: [restKey(W), subKey(W, `${W}/old`), workerKey("old1")],
      limits: {},
    });
    // Already open and within limits: nothing to do.
    expect(reveal(sbWithWorkers, "w-a6", { [workerKey("a6")]: true }, { [groupKey("/a")]: 7 })).toBeNull();
    // Unlinked Worker has no row in Project list.
    expect(reveal(sbWithWorkers, "w-unlinked", {}, {})).toBeNull();
  });
});

describe("suggested", () => {
  it("ticks Projects active this week that are not parents of others and still exist", () => {
    const list = [
      project("/a"),
      project("/b", NOW - 30 * DAY),
      project(W),
      project(`${W}/x`),
      { ...project("/gone"), missing: true },
    ];
    expect([...suggested(list, NOW)].sort()).toEqual(["/a", `${W}/x`]);
  });
});
