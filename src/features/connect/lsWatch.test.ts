import { describe, expect, it, vi } from "vitest";
import { createLsWatch } from "./lsWatch";
import type { LsEntry } from "./protocol";

const entry = (holder_id: string) => ({ holder_id }) as LsEntry;

function setup() {
  const calls: boolean[] = [];
  const onLs = vi.fn();
  const onError = vi.fn();
  let allowed = true;
  const w = createLsWatch({
    watch: async (on) => void calls.push(on),
    onLs,
    onError: (e) => {
      onError(e);
      allowed = false;
    },
    canWatch: () => allowed,
  });
  return { w, calls, onLs, onError, allow: () => (allowed = true) };
}

describe("createLsWatch", () => {
  it("runs the watch only while wanted, passing on changed lists", () => {
    const { w, calls, onLs } = setup();
    w.list([entry("m1")]);
    expect(onLs).not.toHaveBeenCalled();
    w.setWanted(true);
    expect(calls).toEqual([true]);
    w.list([entry("m1")]);
    w.list([entry("m1")]);
    expect(onLs).toHaveBeenCalledTimes(1);
    w.setWanted(false);
    expect(calls).toEqual([true, false]);
    expect(onLs).toHaveBeenLastCalledWith([]);
    // Sent before the stop: dropped.
    w.list([entry("m2")]);
    expect(onLs).toHaveBeenCalledTimes(2);
  });

  it("reports a watch that ended by itself, and starts again once allowed", () => {
    const { w, calls, onError, allow } = setup();
    w.setWanted(true);
    w.ended("toh 版本太旧");
    expect(onError).toHaveBeenCalledWith(new Error("toh 版本太旧"));
    expect(calls).toEqual([true]);
    allow();
    w.sync();
    expect(calls).toEqual([true, true]);
  });
});
