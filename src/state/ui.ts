import { create } from "zustand";

/** View state shared across features. */
interface Ui {
  /** The right panel: the Subagent Run / Workflow Run under this Agent / Workflow call. */
  panel: { conn: string; toolUseId: string } | null;
  openPanel(conn: string, toolUseId: string): void;
  closePanel(): void;
  /** Expanded tool calls / thinking blocks, by `<conn>:<key>`. */
  expanded: Record<string, boolean>;
  toggle(key: string, open?: boolean): void;
}

export const useUi = create<Ui>((set) => ({
  panel: null,
  openPanel: (conn, toolUseId) => set({ panel: { conn, toolUseId } }),
  closePanel: () => set({ panel: null }),
  expanded: {},
  toggle: (key, open) => set((s) => ({ expanded: { ...s.expanded, [key]: open ?? !s.expanded[key] } })),
}));
