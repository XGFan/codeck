import { describe, expect, it } from "vitest";
import { cycleBounds, cycleEnd, cycleStart, roundToMinute, WEEK_MS } from "./cycle";

const MON_14 = { weekday: 0, minutes: 14 * 60 };
const at = (s: string) => new Date(s);

describe("cycleStart", () => {
  it("回到最近一次重置", () => {
    // 2026-10-15 is a Thursday.
    expect(cycleStart(at("2026-10-15T10:00:00"), MON_14)).toEqual(at("2026-10-12T14:00:00"));
    expect(cycleEnd(at("2026-10-12T14:00:00"))).toEqual(at("2026-10-19T14:00:00"));
  });

  it("重置日当天：重置前属于上一周期，重置时刻起属于新周期", () => {
    expect(cycleStart(at("2026-10-12T13:59:00"), MON_14)).toEqual(at("2026-10-05T14:00:00"));
    expect(cycleStart(at("2026-10-12T14:00:00"), MON_14)).toEqual(at("2026-10-12T14:00:00"));
  });

  it("重置日在今天之后（周日 0 点重置，今天周四）", () => {
    expect(cycleStart(at("2026-10-15T10:00:00"), { weekday: 6, minutes: 0 })).toEqual(at("2026-10-11T00:00:00"));
  });
});

describe("cycleBounds", () => {
  it("weekly 规则沿用 cycleStart / cycleEnd 并支持 offset 翻页", () => {
    const cur = cycleBounds(at("2026-10-15T10:00:00"), { kind: "weekly", reset: MON_14 });
    expect(cur).toEqual({ start: at("2026-10-12T14:00:00"), end: at("2026-10-19T14:00:00") });

    const prev = cycleBounds(at("2026-10-15T10:00:00"), { kind: "weekly", reset: MON_14 }, -1);
    expect(prev).toEqual({ start: at("2026-10-05T14:00:00"), end: at("2026-10-12T14:00:00") });
  });

  it("anchor 规则：now 在锚之前一段内时返回 [anchor - 7d, anchor)", () => {
    const anchorMs = Date.parse("2026-11-30T06:00:00.000Z");
    const now = new Date("2026-11-25T07:00:00.000Z");
    const b = cycleBounds(now, { kind: "anchor", anchorMs });
    expect(b.start.getTime()).toBe(anchorMs - WEEK_MS);
    expect(b.end.getTime()).toBe(anchorMs);
  });

  it("anchor 规则：now 正好在锚上时属于新周期", () => {
    const anchorMs = Date.parse("2026-11-30T06:00:00.000Z");
    expect(cycleBounds(new Date(anchorMs), { kind: "anchor", anchorMs })).toEqual({
      start: new Date(anchorMs),
      end: new Date(anchorMs + WEEK_MS),
    });
    expect(cycleBounds(new Date(anchorMs - 1), { kind: "anchor", anchorMs })).toEqual({
      start: new Date(anchorMs - WEEK_MS),
      end: new Date(anchorMs),
    });
  });

  it("anchor 规则：锚在过去很多周时按 7×24 小时往后推", () => {
    const anchorMs = Date.parse("2026-08-03T06:00:00.000Z");
    const now = new Date(anchorMs + 10 * WEEK_MS + 3_600_000);
    const cur = cycleBounds(now, { kind: "anchor", anchorMs });
    expect(cur.start.getTime()).toBe(anchorMs + 10 * WEEK_MS);
    expect(cur.end.getTime()).toBe(anchorMs + 11 * WEEK_MS);

    const exact = cycleBounds(new Date(anchorMs + 10 * WEEK_MS), { kind: "anchor", anchorMs });
    expect(exact.start.getTime()).toBe(anchorMs + 10 * WEEK_MS);
    expect(exact.end.getTime()).toBe(anchorMs + 11 * WEEK_MS);
  });

  it("anchor 规则：anchor 带毫秒时不舍入，完整保留原值并支持 offset", () => {
    const anchorMs = Date.parse("2026-11-30T05:59:59.789Z");
    const now = new Date("2026-11-25T07:00:00.000Z");
    const cur = cycleBounds(now, { kind: "anchor", anchorMs });
    expect(cur.start.getTime()).toBe(anchorMs - WEEK_MS);
    expect(cur.end.getTime()).toBe(anchorMs);
    expect(cur.start.getTime() % 1000).toBe(789);

    const prev = cycleBounds(now, { kind: "anchor", anchorMs }, -2);
    expect(prev.start.getTime()).toBe(anchorMs - 3 * WEEK_MS);
    expect(prev.end.getTime()).toBe(anchorMs - 2 * WEEK_MS);
  });
});

describe("roundToMinute", () => {
  it("四舍五入到最近一分钟", () => {
    expect(roundToMinute(Date.parse("2026-11-30T05:59:59.999Z")).getTime()).toBe(
      Date.parse("2026-11-30T06:00:00.000Z"),
    );
    expect(roundToMinute(Date.parse("2026-11-30T06:00:29.999Z")).getTime()).toBe(
      Date.parse("2026-11-30T06:00:00.000Z"),
    );
    expect(roundToMinute(Date.parse("2026-11-30T06:00:30.000Z")).getTime()).toBe(
      Date.parse("2026-11-30T06:01:00.000Z"),
    );
  });
});
