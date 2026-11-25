import { describe, expect, it } from "vitest";
import type { SessionUsage } from "@/ipc/bindings";
import { buildDaySessionsView, buildSpanSegments } from "./daySessions";

const start = new Date("2026-11-24T00:00:00").getTime();
const end = new Date("2026-11-25T00:00:00").getTime();
const H = 3_600_000;

const mk = (id: string, cost: number, spans: { startMs: number; endMs: number }[] = []): SessionUsage => ({
  sessionId: id,
  title: `Session ${id}`,
  cost,
  requests: 10,
  spans,
});

describe("buildSpanSegments", () => {
  it("按一天总时长把时段换算为百分比，并裁剪越界与忽略范围外时段", () => {
    const segs = buildSpanSegments(
      [
        { startMs: start - H, endMs: start - 1000 }, // completely before day
        { startMs: start + 6 * H, endMs: start + 12 * H }, // 25%..50%
        { startMs: start + 18 * H, endMs: start + 18 * H }, // 0-width point at 75%
        { startMs: start + 23 * H, endMs: end + H }, // clamped to end
      ],
      start,
      end,
    );
    expect(segs).toHaveLength(3);
    expect(segs[0]!.leftPct).toBeCloseTo(25, 5);
    expect(segs[0]!.widthPct).toBeCloseTo(25, 5);
    expect(segs[1]!.leftPct).toBeCloseTo(75, 5);
    expect(segs[1]!.widthPct).toBe(0);
    expect(segs[2]!.leftPct).toBeCloseTo((23 / 24) * 100, 5);
    expect(segs[2]!.widthPct).toBeCloseTo((1 / 24) * 100, 5);
  });
});

describe("buildDaySessionsView", () => {
  it("为空时返回 0 个与空副标题", () => {
    const v = buildDaySessionsView([], start, end);
    expect(v).toEqual({ total: 0, subtitle: "", rows: [], restCount: 0, restCost: 0 });
  });

  it("不超过 6 个时全部列出、无其余行，副标题只写「这一天有 N 个」", () => {
    const v = buildDaySessionsView([mk("a", 5), mk("b", 20), mk("c", 10)], start, end);
    expect(v.total).toBe(3);
    expect(v.subtitle).toBe("这一天有 3 个");
    expect(v.rows.map((r) => r.sessionId)).toEqual(["b", "c", "a"]);
    expect(v.restCount).toBe(0);
    expect(v.restCost).toBe(0);
  });

  it("超过 6 个时按 Cost 倒序取前 6 个，其余合并计数与 Cost 合计", () => {
    const list = [
      mk("s1", 1),
      mk("s2", 50),
      mk("s3", 12),
      mk("s4", 30),
      mk("s5", 4),
      mk("s6", 20),
      mk("s7", 15),
      mk("s8", 8),
      mk("s9", 2.5),
    ];
    const v = buildDaySessionsView(list, start, end);
    expect(v.total).toBe(9);
    expect(v.subtitle).toBe("这一天有 9 个，按 Cost 列前 6 个");
    expect(v.rows.map((r) => r.sessionId)).toEqual(["s2", "s4", "s6", "s7", "s3", "s8"]);
    expect(v.restCount).toBe(3);
    expect(v.restCost).toBeCloseTo(1 + 4 + 2.5, 5);
  });
});
