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

  it("按名称或路径匹配，最近活跃在前", () => {
    expect(matchProjects(projects, parseQuery("identity")).map((p) => p.id)).toEqual(["/x/identity", "/w/identity"]);
    expect(matchProjects(projects, parseQuery("/w/ identity")).map((p) => p.id)).toEqual(["/w/identity"]);
  });
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
