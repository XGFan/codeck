import { describe, expect, it } from "vitest";
import type { SlashCommand } from "@/ipc/bindings";
import { matchCommands, slashCommand, typedCommand } from "./slash";

const cmd = (name: string, source: SlashCommand["source"] = "builtin"): SlashCommand => ({ name, description: null, source, interactive: false });
const LIST = [cmd("clear"), cmd("compact"), cmd("cost"), cmd("release-notes"), cmd("release", "project"), cmd("kit:review", "plugin"), cmd("review")];

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
