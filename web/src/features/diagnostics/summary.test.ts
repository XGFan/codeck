import { describe, expect, it } from "vitest";
import { versionSummary } from "./index";

describe("versionSummary", () => {
  it("gives the range by version order and how many versions the table lists", () => {
    expect(versionSummary(["2.1.100", "2.1.9", "2.1.76"], 1, true)).toBe(
      "共 3 个版本，从 2.1.9 到 2.1.100；下表只列出有失败行或未知项的 1 个",
    );
  });

  it("says nothing failed only when the page has nothing either", () => {
    expect(versionSummary(["2.1.76"], 0, false)).toBe("共 1 个版本，2.1.76，都没有失败行和未知项");
    // Failed lines or unknown items that no version carries (entries without a version).
    expect(versionSummary(["2.1.76", "2.1.91"], 0, true)).toBe(
      "共 2 个版本，从 2.1.76 到 2.1.91；失败行与未知项都对应不到版本",
    );
  });

  it("handles no versions", () => {
    expect(versionSummary([], 0, true)).toBe("没有记录到版本");
  });
});
