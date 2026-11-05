import { beforeAll, describe, expect, it, vi } from "vitest";
import type { Api } from "./api";
import type { SessionQuery } from "./bindings";

let api: Api;

beforeAll(async () => {
  vi.stubGlobal("window", {});
  api = (await import("./mock")).createMockApi();
});

const base: SessionQuery = { projectIds: [], sort: "lastActive", descending: true, timeRange: null, drill: null };

describe("mock list_sessions", () => {
  it("按 lastActive 倒序，且覆盖 ≥8 个会话", async () => {
    const rows = await api.listSessions(base);
    expect(rows.length).toBeGreaterThanOrEqual(8);
    const times = rows.map((r) => r.lastActiveMs);
    expect(times).toEqual([...times].sort((a, b) => b - a));
  });

  it("按项目过滤，升序排序", async () => {
    const projects = await api.listProjects();
    const orbit = projects.find((p) => p.displayName === "orbit-web")!;
    const rows = await api.listSessions({ ...base, projectIds: [orbit.id], sort: "messages", descending: false });
    expect(rows.every((r) => r.projectId === orbit.id)).toBe(true);
    const counts = rows.map((r) => r.messageCount);
    expect(counts).toEqual([...counts].sort((a, b) => a - b));
  });

  it("get_transcript 支持分支选择与隐藏项", async () => {
    const rich = (await api.listSessions(base)).find((s) => s.subagentCount > 0 && s.live?.status === "busy")!;
    const req = { sessionId: rich.id, scope: { kind: "main" } as const, branchChoices: [], includeHidden: false };
    const main = await api.getTranscript(req);
    expect(main.hiddenCount).toBeGreaterThan(0);
    expect(main.nodes.some((n) => n.hidden)).toBe(false);
    expect(main.branchPoints).toHaveLength(1);
    const alt = await api.getTranscript({
      ...req,
      includeHidden: true,
      branchChoices: [{ anchorKey: main.branchPoints[0]!.anchorKey, headId: "n05x" }],
    });
    expect(alt.nodes.some((n) => n.id === "n05x")).toBe(true);
    expect(alt.nodes.some((n) => n.id === "n05")).toBe(false);
  });
});
