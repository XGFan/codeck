import { describe, expect, it } from "vitest";
import { diffStrings, joinDiffs, rowsFromHunks } from "./diff";

describe("diff", () => {
  it("numbers rows and counts +/-", () => {
    const d = diffStrings("a\nb\nc\n", "a\nB\nc\nd\n");
    expect(d.added).toBe(2);
    expect(d.removed).toBe(1);
    const del = d.rows.find((r) => r.kind === "del")!;
    expect(del).toMatchObject({ oldNo: 2, text: "b" });
    expect(d.rows.find((r) => r.kind === "add" && r.text === "d")).toMatchObject({ newNo: 4 });
  });

  it("uses real file line numbers from structuredPatch hunks and separates hunks", () => {
    const d = rowsFromHunks([
      { oldStart: 41, newStart: 41, lines: [" x", "-old", "+new"] },
      { oldStart: 90, newStart: 90, lines: ["+tail", "\\ No newline at end of file"] },
    ]);
    expect(d.rows.map((r) => r.kind)).toEqual(["ctx", "del", "add", "gap", "add"]);
    expect(d.rows[1]).toMatchObject({ oldNo: 42 });
    expect(d.rows[2]).toMatchObject({ newNo: 42 });
    expect(joinDiffs([d, d]).added).toBe(4);
  });
});
