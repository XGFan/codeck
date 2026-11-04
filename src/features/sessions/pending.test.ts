import { describe, expect, it } from "vitest";
import type { LsEntry } from "@/features/drive/protocol";
import type { ProjectSummary } from "@/ipc/bindings";
import { pendingSessions, projectOfCwd } from "./pending";

const project = (id: string, path: string): ProjectSummary => ({ id, path, displayName: id, missing: false }) as ProjectSummary;
const PROJECTS = [project("mono", "/w/mono"), project("pkg", "/w/mono/packages/pkg"), project("other", "/w/other")];

const entry = (session_id: string | null, cwd: string | null, status: string | null = "idle"): LsEntry => ({
  holder_id: `h-${session_id}`,
  session_id,
  status,
  cwd,
  plugin: true,
  claude_pid: null,
  version: null,
  title: null,
});

const none = { projectIds: [], drill: false };

describe("projectOfCwd", () => {
  it("picks the deepest Project containing the cwd, not a sibling with the same prefix", () => {
    expect(projectOfCwd("/w/mono/packages/pkg/src", PROJECTS)).toBe("pkg");
    expect(projectOfCwd("/w/mono/apps", PROJECTS)).toBe("mono");
    expect(projectOfCwd("/w/mono", PROJECTS)).toBe("mono");
    expect(projectOfCwd("/w/monorepo", PROJECTS)).toBeNull();
    expect(projectOfCwd(null, PROJECTS)).toBeNull();
  });
});

describe("pendingSessions", () => {
  it("lists `ccrpc ls` Sessions not in the index, once each; indexed ones and holders without a Session are left out", () => {
    const ls = [entry("a", "/w/mono", "busy"), entry("b", "/w/other"), entry(null, "/w/x"), entry("a", "/w/mono")];
    expect(pendingSessions(ls, new Set(["b"]), none, PROJECTS)).toEqual([{ id: "a", cwd: "/w/mono", status: "busy" }]);
  });

  it("shows nothing while the indexed set is unknown, or under a stats drill", () => {
    const ls = [entry("a", "/w/mono")];
    expect(pendingSessions(ls, null, none, PROJECTS)).toEqual([]);
    expect(pendingSessions(ls, new Set(), { projectIds: [], drill: true }, PROJECTS)).toEqual([]);
  });

  it("under a Project filter keeps only those whose cwd is in a filtered Project", () => {
    const ls = [entry("a", "/w/mono/packages/pkg"), entry("b", "/w/other/x"), entry("c", "/elsewhere"), entry("d", null)];
    const ids = (projectIds: string[]) => pendingSessions(ls, new Set(), { projectIds, drill: false }, PROJECTS).map((p) => p.id);
    expect(ids(["pkg"])).toEqual(["a"]);
    expect(ids(["mono"])).toEqual([]);
    expect(ids(["other", "pkg"])).toEqual(["a", "b"]);
  });
});
