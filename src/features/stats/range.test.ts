import { describe, expect, it } from "vitest";
import { pageable, resolveRange } from "./range";

// A Thursday morning; the cycle resets on Mondays at 14:00.
const now = new Date("2026-10-15T10:00:00");
const reset = { weekday: 0, minutes: 14 * 60 };
const at = (s: string) => new Date(s).getTime();

describe("resolveRange", () => {
  it("the current cycle runs up to now and says what is left; earlier ones end at their reset", () => {
    const cur = resolveRange({ kind: "cycle", offset: 0 }, now, reset);
    expect(cur.timeRange).toEqual({ fromMs: at("2026-10-12T14:00:00"), toMs: null });
    expect(cur.label).toBe("本周期");
    expect(cur.detail).toBe("10月12日 周一 14:00 至 10月19日 周一 14:00，还剩 4 天 4 小时");
    expect(cur.days).toBe(7);

    const prev = resolveRange({ kind: "cycle", offset: -1 }, now, reset);
    expect(prev.timeRange).toEqual({ fromMs: at("2026-10-05T14:00:00"), toMs: at("2026-10-12T14:00:00") });
    expect(prev.label).toBe("上一周期");
    expect(prev.detail).toBe("10月5日 周一 14:00 至 10月12日 周一 14:00");

    expect(resolveRange({ kind: "cycle", offset: -3 }, now, reset).label).toBe("9月21日起的周期");
  });

  it("months are calendar months, paged across years", () => {
    const cur = resolveRange({ kind: "month", offset: 0 }, now, reset);
    expect(cur.timeRange).toEqual({ fromMs: at("2026-10-01T00:00:00"), toMs: null });
    expect(cur.label).toBe("本月");
    expect(cur.detail).toMatch(/^2026 年 10 月，还剩 16 天 \d+ 小时$/);
    expect(cur.days).toBe(31);

    const back = resolveRange({ kind: "month", offset: -10 }, now, reset);
    expect(back.timeRange).toEqual({ fromMs: at("2025-12-01T00:00:00"), toMs: at("2026-01-01T00:00:00") });
    expect(back.label).toBe("2025 年 12 月");
    expect(resolveRange({ kind: "month", offset: -1 }, now, reset).days).toBe(30);
  });

  it("rolling ranges count back from now; 全部 has no bounds; neither pages", () => {
    const r = resolveRange({ kind: "last30", offset: 0 }, now, reset);
    expect(r.timeRange).toEqual({ fromMs: now.getTime() - 30 * 86_400_000, toMs: null });
    expect(r.label).toBe("最近 30 天");
    expect(resolveRange({ kind: "all", offset: 0 }, now, reset)).toMatchObject({
      timeRange: null,
      days: null,
      label: "全部",
    });
    expect(pageable("last7")).toBe(false);
    expect(pageable("cycle")).toBe(true);
  });
});
