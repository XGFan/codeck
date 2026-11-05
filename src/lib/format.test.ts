import { describe, expect, it } from "vitest";
import { formatCost, formatDuration, formatRelative, formatSpan, formatTokens } from "./format";

describe("formatCost", () => {
  it("小额、常规与大额", () => {
    expect(formatCost(0)).toBe("$0");
    expect(formatCost(0.004)).toBe("<$0.01");
    expect(formatCost(12.345)).toBe("$12.35");
    expect(formatCost(999.994)).toBe("$999.99");
    expect(formatCost(1234.5)).toBe("$1,235");
  });
});

describe("formatTokens", () => {
  it("按量级缩写", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(999)).toBe("999");
    expect(formatTokens(48300)).toBe("48.3K");
    expect(formatTokens(1000)).toBe("1K");
    expect(formatTokens(1_210_000)).toBe("1.21M");
    expect(formatTokens(1_543_000_000)).toBe("1.54B");
    expect(formatTokens(2_400_000_000_000)).toBe("2400B");
  });
  it("进位到 1000 时换下一个单位", () => {
    expect(formatTokens(999_999)).toBe("1M");
    expect(formatTokens(999_996_000)).toBe("1B");
  });
});

describe("formatDuration", () => {
  it("小时/分/秒", () => {
    expect(formatDuration(72 * 60_000)).toBe("1 小时 12 分");
    expect(formatDuration(3_600_000)).toBe("1 小时");
    expect(formatDuration(5 * 60_000)).toBe("5 分");
    expect(formatDuration(30_000)).toBe("30 秒");
    expect(formatDuration(64_000)).toBe("1 分");
  });
  it("细节档精确到秒，不足 1 秒写毫秒", () => {
    expect(formatDuration(64_000, "detail")).toBe("1 分 04 秒");
    expect(formatDuration(72 * 60_000 + 5_000, "detail")).toBe("1 小时 12 分");
    expect(formatDuration(7_500_000, "detail")).toBe("2 小时 5 分");
    expect(formatDuration(1_500, "detail")).toBe("2 秒");
    expect(formatDuration(320, "detail")).toBe("320 毫秒");
    expect(formatDuration(999.6, "detail")).toBe("1 秒");
  });
  it("没有值或为负时为空", () => {
    expect(formatDuration(null)).toBe("");
    expect(formatDuration(-1, "detail")).toBe("");
    expect(formatDuration(Number.NaN)).toBe("");
  });
});

describe("formatRelative", () => {
  const now = new Date(2026, 9, 7, 12, 0, 0); // 周三
  it("各档位", () => {
    expect(formatRelative(new Date(2026, 9, 7, 11, 59, 40), now)).toBe("刚刚");
    expect(formatRelative(new Date(2026, 9, 7, 11, 54, 0), now)).toBe("6 分钟前");
    expect(formatRelative(new Date(2026, 9, 6, 8, 0, 0), now)).toBe("昨天");
    expect(formatRelative(new Date(2026, 9, 5, 8, 0, 0), now)).toBe("周一");
    expect(formatRelative(new Date(2026, 8, 30, 8, 0, 0), now)).toBe("9月30日");
  });
});

describe("formatSpan", () => {
  const at = (m: number, d: number, h = 0) => new Date(2026, m - 1, d, h).getTime();
  it("一天内、整天、跨天", () => {
    expect(formatSpan(at(10, 22, 4), at(10, 22, 5))).toBe("10月22日 04:00–05:00");
    expect(formatSpan(at(10, 22, 18), at(10, 23))).toBe("10月22日 18:00–24:00");
    expect(formatSpan(at(10, 22), at(10, 23))).toBe("10月22日");
    expect(formatSpan(at(10, 19), at(10, 26))).toBe("10月19日–10月25日");
  });
});
