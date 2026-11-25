import { describe, expect, it } from "vitest";
import { parseAnsi, stripAnsi } from "./ansi";

describe("ansi", () => {
  it("parses colors and bold, strips leftovers", () => {
    const spans = parseAnsi("\u001b[1m\u001b[38;5;9mFAIL\u001b[0m ok");
    expect(spans[0]).toMatchObject({ text: "FAIL", bold: true });
    expect(spans[0]!.fg).toMatch(/^rgb\(/);
    expect(spans[1]).toMatchObject({ text: " ok" });
    expect(stripAnsi("\u001b[31mx\u001b[0m")).toBe("x");
  });
  it("lifts pure black", () => {
    expect(parseAnsi("\u001b[30mx")[0]!.fg).not.toBe("rgb(0, 0, 0)");
  });
});
