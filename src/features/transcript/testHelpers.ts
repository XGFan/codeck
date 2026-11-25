import type { Node } from "@/ipc/bindings";
import type { GroupingInput, Row } from "./grouping";

export const ok = (t = "ok") => ({ text: t });

export const input = (nodes: Node[], o: Partial<GroupingInput> = {}): GroupingInput => ({
  nodes,
  branchPoints: [],
  subagents: [],
  workflows: [],
  inherited: null,
  orphanSubagentIds: [],
  showHidden: false,
  inheritedExpanded: false,
  ...o,
});

export const kinds = (rows: Row[]) => rows.map((r) => r.kind);

export const groups = (rows: Row[]) => rows.filter((r): r is Extract<Row, { kind: "group" }> => r.kind === "group");
