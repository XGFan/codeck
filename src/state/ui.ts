import { create } from "zustand";

/** A pasted image waiting to be sent: its saved path and a preview URL. */
export interface DraftImage {
  path: string;
  url: string;
}

/** What is typed in a Session's input box and not sent yet. */
export interface Draft {
  text: string;
  images: DraftImage[];
}

/** View state shared across features. */
interface Ui {
  /** The right panel: the Subagent Run / Workflow Run under this Agent / Workflow call. */
  panel: { conn: string; toolUseId: string } | null;
  openPanel(conn: string, toolUseId: string): void;
  closePanel(): void;
  /** Expanded tool calls / thinking blocks, by `<conn>:<key>`. */
  expanded: Record<string, boolean>;
  toggle(key: string, open?: boolean): void;
  /** Unsent input by Session (Holder ID, or a connection id before it has one): kept across switches and reconnects. */
  drafts: Record<string, Draft>;
  setDraft(key: string, f: (d: Draft) => Draft): void;
}

export const EMPTY_DRAFT: Draft = { text: "", images: [] };

export const useUi = create<Ui>((set) => ({
  panel: null,
  openPanel: (conn, toolUseId) => set({ panel: { conn, toolUseId } }),
  closePanel: () => set({ panel: null }),
  expanded: {},
  toggle: (key, open) => set((s) => ({ expanded: { ...s.expanded, [key]: open ?? !s.expanded[key] } })),
  drafts: {},
  setDraft: (key, f) => set((s) => ({ drafts: { ...s.drafts, [key]: f(s.drafts[key] ?? EMPTY_DRAFT) } })),
}));
