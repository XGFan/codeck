import { describe, expect, it } from "vitest";
import { resumeOffer } from "./resume";

describe("resumeOffer", () => {
  const base = { live: false, cwd: "/work/app", cwdMissing: false, ccrpcError: null };

  it("offers Resume for an ended Session whose directory is there", () => {
    expect(resumeOffer(base)).toEqual({ kind: "enabled" });
  });

  it("hides it while the Session runs (outside clay too): no second claude on the same transcript", () => {
    expect(resumeOffer({ ...base, live: true })).toEqual({ kind: "hidden" });
  });

  it("disables it, saying why, without a directory to start in or without ccrpc", () => {
    expect(resumeOffer({ ...base, cwd: null })).toEqual({ kind: "disabled", why: "不知道这个 Session 的启动目录" });
    expect(resumeOffer({ ...base, cwdMissing: true })).toEqual({ kind: "disabled", why: "目录 /work/app 已不存在" });
    expect(resumeOffer({ ...base, ccrpcError: "运行不了 ccrpc" })).toEqual({ kind: "disabled", why: "运行不了 ccrpc" });
  });
});
