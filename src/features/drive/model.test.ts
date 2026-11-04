import { describe, expect, it } from "vitest";
import { effortLabel, modelFamily, modelLabel } from "./Model";

describe("模型与思考等级的显示", () => {
  it("names models as Claude Code Desktop does; display names and unknown ids stay", () => {
    expect(modelLabel("claude-opus-4-7")).toBe("Opus 4.7");
    expect(modelLabel("claude-fable-5-1[1m]")).toBe("Fable 5.1 (1M)");
    expect(modelLabel("claude-sonnet-4-5-20250929")).toBe("Sonnet 4.5");
    expect(modelLabel("claude-opus-5")).toBe("Opus 5");
    expect(modelLabel("Sonnet 4.5")).toBe("Sonnet 4.5");
    expect(modelLabel("deepseek-flash")).toBe("deepseek-flash");
  });

  it("xhigh reads Extra; the family checks an alias", () => {
    expect(effortLabel("xhigh")).toBe("Extra");
    expect(effortLabel("low")).toBe("Low");
    expect(effortLabel("auto")).toBe("Auto");
    expect(modelFamily("claude-opus-4-7")).toBe("opus");
    expect(modelFamily("Fable 5.1")).toBe("fable");
    expect(modelFamily("deepseek-flash")).toBeNull();
  });
});
