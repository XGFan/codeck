import { describe, expect, it } from "vitest";
import { resumeFailure, resumeOffer } from "./resume";

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

describe("resumeFailure", () => {
  // As on a real Pane (claude 2.1.318, the command wrapped over two rows).
  const screen = [
    'guofan@MBP work % env "$TOH_ARG_1" "$TOH_ARG_2" claude --resume 597821af-f8eb-49',
    "aa-8fd8-cf72e4e0b5c3 --plugin-dir /tmp/p/ccrpc",
    "No conversation found with session ID: 597821af-f8eb-49aa-8fd8-cf72e4e0b5c3",
    "guofan@MBP work %",
    "",
  ];

  it("takes claude's error line", () => {
    expect(resumeFailure(screen, 3)).toBe("No conversation found with session ID: 597821af-f8eb-49aa-8fd8-cf72e4e0b5c3");
  });

  it("finds it under a two-line prompt, or before the prompt is drawn", () => {
    const twoLine = [...screen.slice(0, 3), "~/work on main", "❯", ""];
    expect(resumeFailure(twoLine, 4)).toBe("No conversation found with session ID: 597821af-f8eb-49aa-8fd8-cf72e4e0b5c3");
    expect(resumeFailure(screen.slice(0, 3), 3)).toBe("No conversation found with session ID: 597821af-f8eb-49aa-8fd8-cf72e4e0b5c3");
  });

  it("else the last line above the cursor (the prompt starts there)", () => {
    expect(resumeFailure(["% claude --resume x", "Something unexpected", "% ", ""], 2)).toBe("Something unexpected");
    expect(resumeFailure(["", " "], 0)).toBe("claude 退出了");
  });
});
