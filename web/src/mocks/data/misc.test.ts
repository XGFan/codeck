import { afterEach, describe, expect, it, vi } from "vitest";
import { buildMockStats } from "./misc";
import { sessionData } from "./sessions";

describe("buildMockStats（单日）", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("单日范围里 hourlyModels 按小时加起来等于 hourly，且 sessions 来自已有假数据并至少 8 个", () => {
    const fixedNow = new Date("2026-11-25T14:11:00");
    vi.useFakeTimers();
    vi.setSystemTime(fixedNow);

    const yStart = new Date("2026-11-24T00:00:00").getTime();
    const yEnd = new Date("2026-11-25T00:00:00").getTime() - 1;
    const stats = buildMockStats({ timeRange: { fromMs: yStart, toMs: yEnd } });

    expect(stats.hourly).toHaveLength(24);
    expect(stats.hourlyModels.length).toBeGreaterThan(0);

    const byHour = new Map<number, { cost: number; requests: number }>();
    for (const hm of stats.hourlyModels) {
      const prev = byHour.get(hm.startMs) ?? { cost: 0, requests: 0 };
      prev.cost += hm.cost;
      prev.requests += hm.requests;
      byHour.set(hm.startMs, prev);
    }

    for (const h of stats.hourly) {
      const sum = byHour.get(h.startMs) ?? { cost: 0, requests: 0 };
      expect(sum.requests).toBe(h.requests);
      expect(sum.cost).toBeCloseTo(h.cost, 10);
    }

    const knownIds = new Set(sessionData.map((s) => s.summary.id));
    const knownTitles = new Map(sessionData.map((s) => [s.summary.id, s.summary.title]));
    expect(stats.sessions.length).toBeGreaterThanOrEqual(8);
    for (const s of stats.sessions) {
      expect(knownIds.has(s.sessionId)).toBe(true);
      expect(s.title).toBe(knownTitles.get(s.sessionId));
      expect(s.spans.length).toBeGreaterThan(0);
      for (const sp of s.spans) {
        expect(sp.startMs).toBeGreaterThanOrEqual(yStart);
        expect(sp.endMs).toBeLessThanOrEqual(yEnd);
        expect(sp.startMs).toBeLessThanOrEqual(sp.endMs);
      }
    }

    // Today's spans and hourlyModels stop at now.
    const tStart = new Date("2026-11-25T00:00:00").getTime();
    const tEnd = new Date("2026-11-26T00:00:00").getTime() - 1;
    const todayStats = buildMockStats({ timeRange: { fromMs: tStart, toMs: tEnd } });
    for (const hm of todayStats.hourlyModels) {
      expect(hm.startMs).toBeLessThanOrEqual(fixedNow.getTime());
    }
    for (const s of todayStats.sessions) {
      for (const sp of s.spans) {
        expect(sp.endMs).toBeLessThanOrEqual(fixedNow.getTime());
      }
    }

    // Ranges > 26h return empty hourlyModels & sessions.
    const weekStats = buildMockStats({
      timeRange: { fromMs: yStart - 6 * 86_400_000, toMs: yEnd },
    });
    expect(weekStats.hourlyModels).toEqual([]);
    expect(weekStats.sessions).toEqual([]);
  });
});
