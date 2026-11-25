import { describe, expect, it } from "vitest";
import { forkOffer, forkTitle } from "./fork";

describe("forkTitle", () => {
  it("appends (Fork) to the Origin Session's title", () => {
    expect(forkTitle("排查 Redis 连接池泄漏", [])).toBe("排查 Redis 连接池泄漏 (Fork)");
  });

  it("takes the first free number when a Session has that title already", () => {
    expect(forkTitle("拆分", ["拆分 (Fork)"])).toBe("拆分 (Fork 2)");
    expect(forkTitle("拆分", ["拆分 (Fork)", "拆分 (Fork 2)", "拆分 (Fork 4)"])).toBe("拆分 (Fork 3)");
  });

  it("forks a Fork under its Origin's name, not (Fork) (Fork)", () => {
    expect(forkTitle("拆分 (Fork)", ["拆分 (Fork)"])).toBe("拆分 (Fork 2)");
    expect(forkTitle("拆分 (Fork 3)", [])).toBe("拆分 (Fork)");
  });

  it("keeps it on one line", () => {
    expect(forkTitle("第一行\n第二行  ", [])).toBe("第一行 第二行 (Fork)");
  });
});

describe("forkOffer", () => {
  const base = { cwd: "/work/app", cwdMissing: false, ccrpcError: null };

  it("is offered with a directory to start in and ccrpc, also while the Session runs", () => {
    expect(forkOffer(base)).toEqual({ kind: "enabled" });
  });

  it("says why it is not", () => {
    expect(forkOffer({ ...base, cwd: null })).toEqual({ kind: "disabled", why: "不知道这个 Session 的启动目录" });
    expect(forkOffer({ ...base, cwdMissing: true })).toEqual({ kind: "disabled", why: "目录 /work/app 已不存在" });
    expect(forkOffer({ ...base, ccrpcError: "运行不了 ccrpc" })).toEqual({ kind: "disabled", why: "运行不了 ccrpc" });
  });
});
