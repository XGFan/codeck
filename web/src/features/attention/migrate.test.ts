import { describe, expect, it } from "vitest";
import { legacyDelays } from "./migrate";

describe("legacyDelays", () => {
  it("turns the saved milliseconds into minutes, null staying off", () => {
    expect(legacyDelays(JSON.stringify({ waiting: 0, done: null }))).toEqual({ waitingMinutes: 0, doneMinutes: null });
    expect(legacyDelays(JSON.stringify({ waiting: 600_000, done: 60_000 }))).toEqual({
      waitingMinutes: 10,
      doneMinutes: 1,
    });
  });

  it("leaves out what was not saved or makes no sense, and ignores what is no JSON object", () => {
    expect(legacyDelays(JSON.stringify({ waiting: -1 }))).toEqual({});
    expect(legacyDelays(null)).toBeNull();
    expect(legacyDelays("{not json")).toBeNull();
    expect(legacyDelays("3")).toBeNull();
  });
});
