import { describe, expect, it } from "vitest";
import { effortLabel, modelFamily } from "./Model";

describe("模型与思考等级的显示", () => {
  it("xhigh reads Extra; the family reads the id, else the name", () => {
    expect(effortLabel("xhigh")).toBe("Extra");
    expect(effortLabel("low")).toBe("Low");
    expect(effortLabel("auto")).toBe("Auto");
    expect(modelFamily({ id: "claude-opus-4-7", name: "Opus 4.7" })).toBe("opus");
    expect(modelFamily({ id: null, name: "Fable 5.1" })).toBe("fable");
    expect(modelFamily({ id: null, name: "deepseek-flash" })).toBeNull();
    expect(modelFamily(null)).toBeNull();
  });
});
