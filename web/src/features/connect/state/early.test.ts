import { describe, expect, it } from "vitest";
import { createEarlyBuffer, type Early } from "./early";

const line = (n: number): Early => ({
  kind: "line",
  msg: { type: "progress", text: String(n), verb: "", elapsed_s: n, tokens: null },
});
const exit: Early = { kind: "exit", code: 1, stderr: "refused" };

describe("early buffer", () => {
  it("gives back what came before the connection was known, in order, once", () => {
    const b = createEarlyBuffer();
    b.push("c1", line(1));
    b.push("c2", line(9));
    b.push("c1", exit);
    expect(b.take("c1")).toEqual([line(1), exit]);
    expect(b.take("c1")).toEqual([]);
    expect(b.take("c2")).toEqual([line(9)]);
  });

  it("keeps the newest per id (the exit survives) and drops the oldest id past the limit", () => {
    const b = createEarlyBuffer(2, 2);
    b.push("a", line(1));
    b.push("a", line(2));
    b.push("a", exit);
    expect(b.take("a")).toEqual([line(2), exit]);
    b.push("x", line(1));
    b.push("y", line(2));
    b.push("z", line(3));
    expect(b.take("x")).toEqual([]);
    expect(b.take("z")).toEqual([line(3)]);
  });
});
