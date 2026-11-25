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

describe("mock 与后端一致的数据目录与重建", () => {
  it("set_data_root：空白回到默认，相对路径按目录不存在拒绝，之后发 Live 与 Session 变化", async () => {
    const live = vi.fn();
    const changed = vi.fn();
    await api.onLiveChanged(live);
    await api.onSessionsChanged(changed);
    await expect(api.setDataRoot("relative/dir")).rejects.toEqual({
      code: "invalidQuery",
      message: "目录不存在：relative/dir",
    });
    const info = await api.setDataRoot("  ");
    expect(info.dataRootSource).toBe("default");
    await vi.waitFor(() => expect(changed).toHaveBeenCalledWith(expect.objectContaining({ projectsChanged: true })));
    expect(live).toHaveBeenCalled();
  });

  it("rebuild_index 完成时发 projectsChanged 的 sessionsChanged", async () => {
    const changed = vi.fn();
    await api.onSessionsChanged(changed);
    await api.rebuildIndex();
    await vi.waitFor(() => expect(changed).toHaveBeenCalledWith(expect.objectContaining({ projectsChanged: true })));
  });
});

describe("mock get_stats 统一逐小时种子与守恒", () => {
  it("同一完整自然日在短范围（含 hourly）与长范围（上月 / 最近 90 天）里数值完全一致，且 hourly 与 daily 守恒", async () => {
    const DAY = 86_400_000;
    const anchor = Date.now();
    // Pick a complete calendar day 5 days ago (strictly inside a 7-day, 30-day, and 90-day range).
    const targetDate = new Date(anchor - 5 * DAY);
    const targetDay = `${targetDate.getFullYear()}-${String(targetDate.getMonth() + 1).padStart(2, "0")}-${String(targetDate.getDate()).padStart(2, "0")}`;

    const shortRange = { fromMs: anchor - 7 * DAY, toMs: anchor - 1 };
    const monthRange = { fromMs: anchor - 30 * DAY, toMs: anchor - 1 };
    const ninetyRange = { fromMs: anchor - 90 * DAY, toMs: anchor - 1 };

    const [sShort, sMonth, sNinety] = await Promise.all([
      api.getStats({ timeRange: shortRange }),
      api.getStats({ timeRange: monthRange }),
      api.getStats({ timeRange: ninetyRange }),
    ]);

    const pick = (daily: typeof sShort.daily) => daily.filter((d) => d.day === targetDay);
    expect(pick(sShort.daily).length).toBeGreaterThan(0);
    expect(pick(sShort.daily)).toEqual(pick(sMonth.daily));
    expect(pick(sShort.daily)).toEqual(pick(sNinety.daily));

    // Short range hourly conservation against daily & overview.
    expect(sShort.hourly.length).toBeGreaterThan(0);
    expect(sMonth.hourly).toHaveLength(0);
    expect(sShort.hourly.reduce((s, h) => s + h.requests, 0)).toBe(sShort.overview.requests);
    expect(sShort.daily.reduce((s, d) => s + d.requests, 0)).toBe(sShort.overview.requests);
    expect(sShort.hourly.reduce((s, h) => s + h.cost, 0)).toBeCloseTo(sShort.overview.cost, 6);
  });
});
