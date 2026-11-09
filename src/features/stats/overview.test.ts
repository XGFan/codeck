import { describe, expect, it } from "vitest";
import { inputTotals, longestStreak, perDay } from "./overview";

describe("longestStreak", () => {
  it("counts the longest run of consecutive days, in any order and with repeats", () => {
    expect(longestStreak([])).toBe(0);
    expect(longestStreak(["2026-10-03", "2026-10-01", "2026-10-02", "2026-10-02", "2026-10-05"])).toBe(3);
  });

  it("runs across month and year ends", () => {
    expect(longestStreak(["2026-12-31", "2027-01-01", "2026-12-30"])).toBe(3);
    expect(longestStreak(["2026-02-28", "2026-03-01"])).toBe(2);
  });
});

describe("inputTotals", () => {
  it("adds up uncached input, cache reads and cache writes over every model's rows", () => {
    const row = (input: number, cacheRead: number, cacheCreation: number) => ({
      day: "2026-10-01",
      model: "m",
      input,
      output: 999,
      cacheRead,
      cacheCreation,
      cost: 0,
    });
    expect(inputTotals([row(1, 10, 100), row(2, 20, 200)])).toEqual({ input: 3, read: 30, write: 300 });
    expect(inputTotals([])).toEqual({ input: 0, read: 0, write: 0 });
  });
});

describe("perDay", () => {
  it("keeps one decimal below 10, so a quiet range does not read as 0", () => {
    expect(perDay(0.25)).toBe("0.3");
    expect(perDay(3)).toBe("3");
    expect(perDay(12.6)).toBe("13");
  });
});
