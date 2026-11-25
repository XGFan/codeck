import type { SearchGroup, SearchHit } from "@/ipc/bindings";

/** Content hits shown per Session. */
export const HITS_PER_SESSION = 5;

/** `i` (the hit's place in its group) keeps two hits in different blocks of one entry apart. */
export const hitKey = (sessionId: string, h: SearchHit, i: number) =>
  `h:${sessionId}|${h.nodeId}|${h.agentId ?? ""}|${h.toolUseId ?? ""}|${h.role}|${i}`;

/**
 * Every selectable value in display order, by section (Projects `p:`, Sessions `s:`, content hits `h:`); empty
 * sections are left out. Hits of an earlier query (`stale`) are shown but not selectable.
 */
export function selectableSections(
  projectIds: string[],
  sessionIds: string[],
  groups: SearchGroup[],
  stale: boolean,
): string[][] {
  return [
    projectIds.map((id) => `p:${id}`),
    sessionIds.map((id) => `s:${id}`),
    stale ? [] : groups.flatMap((g) => g.hits.slice(0, HITS_PER_SESSION).map((h, i) => hitKey(g.session.id, h, i))),
  ].filter((keys) => keys.length > 0);
}

/** ⇥ / ⇧⇥: the first value of the next (previous) section, wrapping around; from no selection, the first one. */
export function nextSection(sections: string[][], selected: string, back: boolean): string | undefined {
  if (sections.length === 0) return undefined;
  const at = sections.findIndex((keys) => keys.includes(selected));
  const next = (at + (back ? -1 : 1) + sections.length) % sections.length;
  return sections[next]![0];
}

/** Who wrote the hit: the Subagent's type, you, Claude, or a tool's input. */
export function hitRole(hit: SearchHit): string {
  if (hit.agentId) return hit.agentType ?? "Subagent";
  return hit.role === "user" ? "你" : hit.role === "assistant" ? "Claude" : "工具输入";
}
