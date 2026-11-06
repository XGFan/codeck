import { describe, expect, it } from "vitest";
import type { ProjectSummary, SessionSummary } from "@/ipc/bindings";
import { buildSidebar, flatten, groupKey, OTHER_KEY, restKey, reveal, subKey, suggested } from "./groups";

const NOW = Date.UTC(2026, 10, 6, 12);
const MIN = 60_000;
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
  it("groups by the Project a Session started in and orders by the latest user input, not the latest reply", () => {
    const sb = buildSidebar(
      [
        // Replied to just now, prompted long ago: stays behind.
        session("a-old", "/a", NOW - 3 * DAY, { lastActiveMs: NOW }),
        session("a-new", "/a", NOW - DAY),
        session("b1", "/b", NOW - 2 * DAY),
        session("w-root", W, NOW - 5 * DAY),
      ],
      [],
      projects,
      [{ path: "/a" }, { path: "/b" }],
      NOW,
    );
    expect(sb.groups.map((g) => g.path)).toEqual(["/a", "/b"]);
    expect(ids(sb.groups[0]!.sessions)).toEqual(["a-new", "a-old"]);
    expect(ids(sb.other)).toEqual(["w-root"]);
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
    // Ordered by the newest Session anywhere in the group.
    expect(nested.groups[0]!.path).toBe(W);

    const flat = buildSidebar(sessions, [], projects, [{ path: W }], NOW);
    expect(ids(flat.groups[0]!.sessions)).toEqual(["root"]);
    expect(ids(flat.other)).toEqual(["y1", "x1", "old1"]);
  });

  it("lists Live and pending Sessions under 「运行中」; a pending one joins the group its cwd is in", () => {
    const sb = buildSidebar(
      [session("a1", "/a", NOW - DAY, { live: live("waiting") }), session("a2", "/a", NOW - 2 * DAY)],
      [{ id: "p1", cwd: "/a/sub", status: "idle" }],
      projects,
      [{ path: "/a" }],
      NOW,
    );
    expect(ids(sb.running)).toEqual(["p1", "a1"]);
    expect(ids(sb.groups[0]!.sessions)).toEqual(["p1", "a1", "a2"]);
    expect(sb.groups[0]!.live).toBe("waiting");
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
  const names = new Map(projects.map((p) => [p.id, p.displayName]));

  it("shows 5 rows then 「显示更多」; sub-groups and 「其他」 start closed, inactive sub-Projects folded", () => {
    const items = flatten(sb, {}, {}, names);
    expect(items.map((i) => i.kind)).toEqual([
      "section",
      "group",
      ...Array(5).fill("row"),
      "more",
      "group",
      "sub",
      "subsRest",
      "other",
    ]);
    expect(items.find((i) => i.kind === "more")).toMatchObject({ rest: 2 });
    expect(items.find((i) => i.kind === "subsRest")).toMatchObject({ count: 1 });
  });

  it("follows the open state and 「显示更多」, tags 「其他」's rows with their Project", () => {
    const items = flatten(
      sb,
      { [groupKey("/a")]: false, [subKey(W, `${W}/y`)]: true, [restKey(W)]: true, [OTHER_KEY]: true },
      {},
      names,
    );
    const rows = items.filter((i) => i.kind === "row");
    expect(rows.map((r) => [r.s.id, r.depth, r.tag])).toEqual([
      ["y1", 2, null],
      ["b1", 1, "b"],
    ]);
    expect(items.filter((i) => i.kind === "sub").map((s) => s.sub.projectId)).toEqual([`${W}/y`, `${W}/old`]);
    expect(flatten(sb, {}, { [groupKey("/a")]: 25 }, names).filter((i) => i.kind === "row")).toHaveLength(7);
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
