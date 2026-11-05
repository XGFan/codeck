import { describe, expect, it } from "vitest";
import type { SlashCommand } from "@/ipc/bindings";
import { applyPath, matchCommands, pathAt, slashCommand, typedCommand } from "./slash";

const cmd = (name: string, source: SlashCommand["source"] = "builtin"): SlashCommand => ({
  name,
  description: null,
  source,
  interactive: false,
});
const LIST = [
  cmd("clear"),
  cmd("compact"),
  cmd("cost"),
  cmd("release-notes"),
  cmd("release", "project"),
  cmd("kit:review", "plugin"),
  cmd("review"),
];

describe("typedCommand", () => {
  it("is the name while only `/name` is typed", () => {
    expect(typedCommand("/")).toBe("");
    expect(typedCommand("/co")).toBe("co");
    expect(typedCommand("/kit:re")).toBe("kit:re");
    expect(typedCommand("/compact now")).toBeNull();
    expect(typedCommand("/co\nx")).toBeNull();
    expect(typedCommand("/Users/me")).toBeNull();
    expect(typedCommand("hi /co")).toBeNull();
  });
});

describe("matchCommands", () => {
  it("ranks prefix, then namespaced part, then substring matches", () => {
    expect(matchCommands(LIST, "re").map((c) => c.name)).toEqual(["release-notes", "release", "review", "kit:review"]);
    expect(matchCommands(LIST, "view").map((c) => c.name)).toEqual(["kit:review", "review"]);
    expect(matchCommands(LIST, "CO").map((c) => c.name)).toEqual(["compact", "cost"]);
    expect(matchCommands(LIST, "", 2).map((c) => c.name)).toEqual(["clear", "compact"]);
    expect(matchCommands(LIST, "release").map((c) => c.name)).toEqual(["release", "release-notes"]);
  });
});

describe("slashCommand", () => {
  it("sends built-in and unknown commands as `slash`, custom ones and the rest as `prompt`", () => {
    expect(slashCommand("/compact keep the API notes", false, LIST)).toBe("compact keep the API notes");
    expect(slashCommand("/doctor", false, LIST)).toBe("doctor");
    expect(slashCommand("/cost", false, undefined)).toBeNull();
    expect(slashCommand("/Users/me/shot.png 这是什么", false, LIST)).toBeNull();
    expect(slashCommand("/release v1", false, LIST)).toBeNull();
    expect(slashCommand("/kit:review", false, LIST)).toBeNull();
    expect(slashCommand("/compact\nmore", false, LIST)).toBeNull();
    expect(slashCommand("/cost", true, LIST)).toBeNull();
    expect(slashCommand("/", false, LIST)).toBeNull();
    expect(slashCommand("hello", false, LIST)).toBeNull();
  });
});

describe("pathAt / applyPath", () => {
  it("/add-dir completes its whole argument, directories only", () => {
    expect(pathAt("/add-dir ../in", 3)).toEqual({ partial: "../in", start: 9, end: 14, files: false });
    expect(pathAt("/add-dir", 8)).toBeNull();
  });

  it("@ completes the word the caret is in, files too", () => {
    const t = "看看 @src/ma 和 @docs";
    expect(pathAt(t, 10)).toEqual({ partial: "src/ma", start: 4, end: 10, files: true });
    // Caret inside the word: the word up to its end.
    expect(pathAt(t, 7)).toEqual({ partial: "src/ma", start: 4, end: 10, files: true });
    expect(pathAt("@", 1)).toEqual({ partial: "", start: 1, end: 1, files: true });
    // Not after a letter (an email), not past a space.
    expect(pathAt("me@host", 7)).toBeNull();
    expect(pathAt("@src next", 9)).toBeNull();
  });

  it("a file is followed by a space, a directory is not; the caret goes after it", () => {
    const p = pathAt("看 @src/ma 吧", 9)!;
    expect(applyPath("看 @src/ma 吧", p, "src/main.rs")).toEqual({ text: "看 @src/main.rs 吧", caret: 15 });
    const q = pathAt("看 @sr", 5)!;
    expect(applyPath("看 @sr", q, "src/")).toEqual({ text: "看 @src/", caret: 7 });
    expect(applyPath("看 @sr", q, "src/a.rs")).toEqual({ text: "看 @src/a.rs ", caret: 12 });
    const d = pathAt("/add-dir ap", 11)!;
    expect(applyPath("/add-dir ap", d, "apps/")).toEqual({ text: "/add-dir apps/", caret: 14 });
  });
});
