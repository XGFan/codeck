import { create } from "zustand";
import type { BranchChoice, Drill, JumpTarget, SessionSort, TimeRange, TranscriptScope } from "@/ipc/bindings";
import type { WeeklyReset } from "@/lib/cycle";

export type CollapsiblePane = "sessions";

const COLLAPSED_KEY = "codeck.collapsedPanes";
const WEEKLY_RESET_KEY = "codeck.weeklyReset";
const CONV_WIDTH_KEY = "codeck.conversationWidth";

/** The conversation column's width (px): dragged at its edges, centered; double-click goes back to the default. */
export const CONV_WIDTH = { default: 720, min: 480, max: 1600 };

/** A remembered preference over `fallback`; `fallback` alone when storage is unavailable or garbled. */
function load<T extends object>(key: string, fallback: T): T {
  try {
    return { ...fallback, ...JSON.parse(localStorage.getItem(key) ?? "{}") };
  } catch {
    return fallback;
  }
}

function clampWidth(px: number): number {
  return Number.isFinite(px) ? Math.round(Math.min(CONV_WIDTH.max, Math.max(CONV_WIDTH.min, px))) : CONV_WIDTH.default;
}

function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Not remembered; the change still applies to this window.
  }
}

export interface Highlight {
  nodeId: string;
  toolUseId?: string;
}

/** Per-session state; reset whenever `sessionId` changes. */
const sessionDefaults = {
  scope: { kind: "main" } as TranscriptScope,
  branchChoices: [] as BranchChoice[],
  showHidden: false,
  /** Subagent Run ids, outermost first. */
  panelStack: [] as string[],
  findOpen: false,
  findQuery: "",
  findIndex: 0,
  highlight: null as Highlight | null,
  /** Keys: `tool:<nodeId>|<toolUseId>`, `group:<firstNodeId>|<firstToolUseId>`, `thinking:<nodeId>`, `inherited`, `compact:<nodeId>`. */
  expanded: {} as Record<string, boolean>,
};

interface UiState {
  /** Session list filter; empty = all projects (matches `SessionQuery.projectIds`). */
  projectIds: string[];
  sessionId: string | null;
  scope: TranscriptScope;
  branchChoices: BranchChoice[];
  showHidden: boolean;
  panelStack: string[];
  searchOpen: boolean;
  /** The stats dialog. */
  statsOpen: boolean;
  findOpen: boolean;
  findQuery: string;
  findIndex: number;
  pendingJump: JumpTarget | null;
  highlight: Highlight | null;
  expanded: Record<string, boolean>;
  sessionSort: SessionSort;
  sessionDescending: boolean;
  statsDrill: Drill | null;
  /** Time range of the stats view a drill came from; applied to the session list only while the drill is active. */
  statsDrillRange: TimeRange | null;
  settingsOpen: boolean;
  /** User-collapsed columns; remembered across launches. */
  collapsed: Record<CollapsiblePane, boolean>;
  /** The conversation column's width (`CONV_WIDTH`); remembered across launches. */
  convWidth: number;
  setConvWidth: (px: number) => void;
  /** When the stats page's weekly cycle starts over; remembered across launches. */
  weeklyReset: WeeklyReset;

  setProjectIds: (ids: string[]) => void;
  /** Switches session (resetting per-session state); a no-op when the id is unchanged. */
  selectSession: (id: string | null) => void;
  /** Selects `headId` at `anchorKey`, replacing any earlier choice for that anchor. */
  setBranchChoice: (choice: BranchChoice) => void;
  setShowHidden: (show: boolean) => void;
  setPanelStack: (stack: string[]) => void;
  pushPanel: (agentId: string) => void;
  popPanel: () => void;
  setSearchOpen: (open: boolean) => void;
  setStatsOpen: (open: boolean) => void;
  setFindOpen: (open: boolean) => void;
  setFindQuery: (query: string) => void;
  setFindIndex: (index: number) => void;
  /** Opens the target's session (if different) and queues the jump for the transcript to consume. */
  jumpTo: (target: JumpTarget) => void;
  setHighlight: (h: Highlight | null) => void;
  toggleExpanded: (key: string, defaultValue?: boolean) => void;
  setExpanded: (key: string, value: boolean) => void;
  setSessionSort: (sort: SessionSort, descending?: boolean) => void;
  setStatsDrill: (drill: Drill | null, range?: TimeRange | null) => void;
  setSettingsOpen: (open: boolean) => void;
  toggleCollapsed: (pane: CollapsiblePane) => void;
  setWeeklyReset: (reset: WeeklyReset) => void;
}

export const useUi = create<UiState>()((set) => ({
  projectIds: [],
  sessionId: null,
  ...sessionDefaults,
  searchOpen: false,
  statsOpen: false,
  pendingJump: null,
  sessionSort: "lastActive",
  sessionDescending: true,
  statsDrill: null,
  statsDrillRange: null,
  settingsOpen: false,
  collapsed: load(COLLAPSED_KEY, { sessions: false }),
  weeklyReset: load(WEEKLY_RESET_KEY, { weekday: 0, minutes: 0 }),
  convWidth: clampWidth(load(CONV_WIDTH_KEY, { px: CONV_WIDTH.default }).px),
  setConvWidth: (px) => {
    const convWidth = clampWidth(px);
    save(CONV_WIDTH_KEY, { px: convWidth });
    set({ convWidth });
  },

  setProjectIds: (projectIds) => set({ projectIds }),
  selectSession: (id) => set((s) => (s.sessionId === id ? s : { sessionId: id, ...sessionDefaults })),
  setBranchChoice: (choice) =>
    set((s) => ({ branchChoices: [...s.branchChoices.filter((c) => c.anchorKey !== choice.anchorKey), choice] })),
  setShowHidden: (showHidden) => set({ showHidden }),
  setPanelStack: (panelStack) => set({ panelStack }),
  pushPanel: (agentId) => set((s) => ({ panelStack: [...s.panelStack, agentId] })),
  popPanel: () => set((s) => ({ panelStack: s.panelStack.slice(0, -1) })),
  setSearchOpen: (searchOpen) => set({ searchOpen }),
  setStatsOpen: (statsOpen) => set({ statsOpen }),
  setFindOpen: (findOpen) => set({ findOpen }),
  setFindQuery: (findQuery) => set({ findQuery, findIndex: 0 }),
  setFindIndex: (findIndex) => set({ findIndex }),
  jumpTo: (target) =>
    set((s) => ({
      pendingJump: target,
      ...(s.sessionId === target.sessionId ? {} : { sessionId: target.sessionId, ...sessionDefaults }),
    })),
  setHighlight: (highlight) => set({ highlight }),
  toggleExpanded: (key, defaultValue = false) =>
    set((s) => ({ expanded: { ...s.expanded, [key]: !(s.expanded[key] ?? defaultValue) } })),
  setExpanded: (key, value) => set((s) => ({ expanded: { ...s.expanded, [key]: value } })),
  setSessionSort: (sessionSort, descending) =>
    set((s) => ({ sessionSort, sessionDescending: descending ?? s.sessionDescending })),
  setStatsDrill: (statsDrill, range = null) => set({ statsDrill, statsDrillRange: statsDrill ? range : null }),
  setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
  toggleCollapsed: (pane) =>
    set((s) => {
      const collapsed = { ...s.collapsed, [pane]: !s.collapsed[pane] };
      save(COLLAPSED_KEY, collapsed);
      return { collapsed };
    }),
  setWeeklyReset: (weeklyReset) => {
    save(WEEKLY_RESET_KEY, weeklyReset);
    set({ weeklyReset });
  },
}));
