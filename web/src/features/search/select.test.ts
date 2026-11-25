import { describe, expect, it } from "vitest";
import type { SearchGroup, SearchHit, SessionSummary } from "@/ipc/bindings";
import { hitKey, hitRole, nextSection, selectableSections } from "./select";

const hit = (nodeId: string, more: Partial<SearchHit> = {}): SearchHit => ({
  nodeId,
  agentId: null,
  agentType: null,
  toolUseId: null,
  role: "assistant",
  timestampMs: null,
  onMainLine: true,
  snippet: [],
  ...more,
});
const group = (id: string, hits: SearchHit[]) =>
  ({ session: { id } as SessionSummary, hitCount: hits.length, hits }) as SearchGroup;

describe("selectableSections", () => {
  it("lists Projects, Sessions and content hits in display order, leaving out empty sections", () => {
    const groups = [group("a", [hit("n1"), hit("n2")])];
    expect(selectableSections(["p1"], ["s1", "s2"], groups, false)).toEqual([
      ["p:p1"],
      ["s:s1", "s:s2"],
      [hitKey("a", hit("n1"), 0), hitKey("a", hit("n2"), 1)],
    ]);
    expect(selectableSections([], ["s1"], [], false)).toEqual([["s:s1"]]);
  });

  it("takes the first five hits of a Session and none from an earlier query", () => {
    const many = [
      group(
        "a",
        Array.from({ length: 7 }, (_, i) => hit(`n${i}`)),
      ),
    ];
    expect(selectableSections([], [], many, false)[0]).toHaveLength(5);
    expect(selectableSections([], ["s1"], many, true)).toEqual([["s:s1"]]);
  });

  it("keeps two hits on one node apart by their place in the group", () => {
    expect(hitKey("a", hit("n"), 0)).not.toBe(hitKey("a", hit("n"), 1));
  });
});

describe("nextSection", () => {
  const sections = [["p:1"], ["s:1", "s:2"], ["h:1"]];

  it("moves to the first value of the next section and wraps around", () => {
    expect(nextSection(sections, "s:2", false)).toBe("h:1");
    expect(nextSection(sections, "h:1", false)).toBe("p:1");
    expect(nextSection(sections, "p:1", true)).toBe("h:1");
    expect(nextSection(sections, "h:1", true)).toBe("s:1");
  });

  it("starts from the first section when nothing is selected; nothing to move to without sections", () => {
    expect(nextSection(sections, "", false)).toBe("p:1");
    expect(nextSection([], "", false)).toBeUndefined();
  });
});

describe("hitRole", () => {
  it("names the Subagent's type, you, Claude or a tool's input", () => {
    expect(hitRole(hit("n", { agentId: "x", agentType: "Explore" }))).toBe("Explore");
    expect(hitRole(hit("n", { agentId: "x", role: "user" }))).toBe("Subagent");
    expect(hitRole(hit("n", { role: "user" }))).toBe("你");
    expect(hitRole(hit("n"))).toBe("Claude");
    expect(hitRole(hit("n", { role: "toolInput" }))).toBe("工具输入");
  });
});
