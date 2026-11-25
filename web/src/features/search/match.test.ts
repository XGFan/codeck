import { describe, expect, it } from "vitest";
import type { ProjectSummary, SessionSummary } from "@/ipc/bindings";
import { highlight, matchProjects, matchSessions, parseQuery, sessionProjectIds } from "./match";

const session = (
  id: string,
  title: string,
  projectId: string,
  editedProjectIds: string[] = [],
  gitBranch: string | null = null,
) => ({ id, title, projectId, editedProjectIds, gitBranch }) as SessionSummary;

const names = new Map([
  ["/w", "richemont"],
  ["/w/identity", "rwf-identity"],
  ["/w/task", "rwf-task"],
  ["/w/wmc", "rwf-wmc"],
]);

describe("parseQuery", () => {
  it("与全文搜索同一套语法：空格、短语、排除", () => {
    expect(parseQuery(`Rate  "token bucket" -redis -"connection pool" a-b`)).toEqual({
      include: ["rate", "token bucket", "a-b"],
      exclude: ["redis", "connection pool"],
    });
    expect(parseQuery(`"未闭合 短语`)).toEqual({ include: ["未闭合 短语"], exclude: [] });
    expect(parseQuery("- x")).toEqual({ include: ["-", "x"], exclude: [] });
  });
});

describe("sessionProjectIds", () => {
  it("启动 Project 在前，改动过的其他 Project 随后且不重复", () => {
    expect(sessionProjectIds(session("a", "", "/w/identity", ["/w/task", "/w/identity"]))).toEqual([
      "/w/identity",
      "/w/task",
    ]);
  });
});

describe("matchSessions", () => {
  const list = [
    session("newest", "修复 identity 登录", "/w/wmc"),
    session("multi", "排查回调失败", "/w", ["/w/identity", "/w/task"]),
    session("branch", "改字段", "/w/wmc", [], "WESP-6195"),
  ];
  const ids = (q: string) => matchSessions(list, names, parseQuery(q)).map((s) => s.id);

  it("每个词都要在标题、Project 名或分支里出现", () => {
    expect(ids("identity task")).toEqual(["multi"]);
    expect(ids("wesp")).toEqual(["branch"]);
    expect(ids("登录 wmc")).toEqual(["newest"]);
  });
  it("命中 Project 名或分支的排在只命中标题的前面", () => {
    expect(ids("identity")).toEqual(["multi", "newest"]);
  });
  it("排除词出现在任一字段都排除；只有排除词时不匹配", () => {
    expect(ids("identity -task")).toEqual(["newest"]);
    expect(ids("-task")).toEqual([]);
  });
});

describe("matchProjects", () => {
  const projects = [
    { id: "/w/identity", path: "/w/identity", displayName: "rwf-identity", lastActiveMs: 1 },
    { id: "/x/identity", path: "/x/identity", displayName: "identity", lastActiveMs: 2 },
    { id: "/w/wmc", path: "/w/wmc", displayName: "rwf-wmc", lastActiveMs: 3 },
  ] as ProjectSummary[];

  it("按名称或路径匹配", () => {
    expect(matchProjects(projects, parseQuery("/w/ identity")).map((p) => p.id)).toEqual(["/w/identity"]);
  });
  it("名称等于查询的在前，其次名称命中的，只有路径命中的最后；同档最近活跃在前", () => {
    const joy = [
      { id: "/d/Joy/ford", path: "/d/Joy/ford", displayName: "ford", lastActiveMs: 9 },
      { id: "/f/joy/a", path: "/f/joy/a", displayName: "a", lastActiveMs: 8 },
      { id: "/d/joyride", path: "/d/joyride", displayName: "joyride", lastActiveMs: 7 },
      { id: "/d/Joy", path: "/d/Joy", displayName: "Joy", lastActiveMs: 1 },
    ] as ProjectSummary[];
    expect(matchProjects(joy, parseQuery("Joy")).map((p) => p.id)).toEqual([
      "/d/Joy",
      "/d/joyride",
      "/d/Joy/ford",
      "/f/joy/a",
    ]);
    expect(matchProjects(projects, parseQuery("identity")).map((p) => p.id)).toEqual(["/x/identity", "/w/identity"]);
  });
});

it("Session 与 Project 支持短语和排除词", () => {
  const lumen = "/Users/dev/Developer/lumen-api";
  const orbit = "/Users/dev/Developer/orbit-web";
  const list = [
    session("rate-limit", "为 /v1/chat 接口增加令牌桶限流中间件", lumen),
    session("redis", "排查 Redis 连接池泄漏", lumen),
    session("zod", "迁移表单校验到 zod", orbit, [lumen]),
  ];
  const projectNames = new Map([
    [lumen, "lumen-api"],
    [orbit, "orbit-web"],
  ]);
  const projects: ProjectSummary[] = [
    { id: lumen, path: lumen, displayName: "lumen-api", missing: false, sessionCount: 3, lastActiveMs: 2 },
    { id: orbit, path: orbit, displayName: "orbit-web", missing: false, sessionCount: 1, lastActiveMs: 1 },
  ];
  const excluded = parseQuery("lumen -redis");
  expect(matchProjects(projects, excluded).map((p) => p.id)).toEqual([lumen]);
  expect(matchSessions(list, projectNames, excluded).map((s) => s.id)).toEqual(["rate-limit", "zod"]);
  expect(matchSessions(list, projectNames, parseQuery('"表单校验"')).map((s) => s.id)).toEqual(["zod"]);
});

describe("highlight", () => {
  it("不区分大小写标出所有词，正则字符按字面匹配", () => {
    expect(highlight("Fix ZOD and zod.parse", ["zod.parse", "zod"])).toEqual([
      { text: "Fix ", hit: false },
      { text: "ZOD", hit: true },
      { text: " and ", hit: false },
      { text: "zod.parse", hit: true },
    ]);
    expect(highlight("abc", [])).toEqual([{ text: "abc", hit: false }]);
  });
});
