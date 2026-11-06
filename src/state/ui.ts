import { create } from "zustand";
import type { BranchChoice, Drill, JumpTarget, TimeRange, TranscriptScope } from "@/ipc/bindings";
import type { WeeklyReset } from "@/lib/cycle";

export type CollapsiblePane = "sessions";

const COLLAPSED_KEY = "codeck.collapsedPanes";
const WEEKLY_RESET_KEY = "codeck.weeklyReset";
const CONV_WIDTH_KEY = "codeck.conversationWidth";
const AUTO_CONNECT_KEY = "codeck.autoConnect";
const SIDEBAR_EXPANDED_KEY = "codeck.sidebarExpanded";
const SIDEBAR_WIDTH_KEY = "codeck.sidebarWidth";
const RECENT_ROWS_KEY = "codeck.recentRows";

/** The sidebar's width (px): dragged at its right edge; double-click goes back to the default. */
export const SIDEBAR_WIDTH = { default: 320, min: 240, max: 440 };

/** 「Recent」's rows: dragged at its top edge, whole rows, at least one. */
export const RECENT_ROWS = { default: 6, min: 1, max: 60 };

/** The conversation column's width (px), set in 「设置 › 通用」; centered. */
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

function clamp(n: number, r: { default: number; min: number; max: number }): number {
  return Number.isFinite(n) ? Math.round(Math.min(r.max, Math.max(r.min, n))) : r.default;
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

/** The main conversation (the transcript list); a Subagent Run is shown in its panel with its own scope. */
export const MAIN_SCOPE: TranscriptScope = { kind: "main" };

/** Per-session state; reset whenever `sessionId` changes. */
const sessionDefaults = {
  branchChoices: [] as BranchChoice[],
  showHidden: false,
  /** Subagent Run ids, outermost first. */
  panelStack: [] as string[],
  findOpen: false,
  findQuery: "",
  findIndex: 0,
  highlight: null as Highlight | null,
  /** Expansion state, by the keys built in `features/transcript/keys.ts`. */
  expanded: {} as Record<string, boolean>,
};

/** A dialog over the window is open (⌘K, stats, Settings, the sidebar's): keys meant for the window behind are not handled. */
export const selectOverlayOpen = (
  s: Pick<UiState, "searchOpen" | "statsOpen" | "settingsOpen" | "newSessionOpen" | "addProjectOpen">,
) => s.searchOpen || s.statsOpen || s.settingsOpen || s.newSessionOpen || s.addProjectOpen;

interface UiState {
  /** Session list filter; empty = all projects (matches `SessionQuery.projectIds`). */
  projectIds: string[];
  sessionId: string | null;
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
  /** Opening a Session that can be Connected connects it (on by default); remembered across launches. */
  autoConnect: boolean;
  setAutoConnect: (on: boolean) => void;
  /** The sidebar's open / closed groups, by the keys in `features/sessions/groups.ts`; remembered across launches. */
  sidebarExpanded: Record<string, boolean>;
  setSidebarExpanded: (changes: Record<string, boolean>) => void;
  /** Rows shown per sidebar list after 「显示更多」 (`FIRST_ROWS` when absent). */
  sidebarLimits: Record<string, number>;
  /** The sidebar's width (`SIDEBAR_WIDTH`); remembered across launches. */
  sidebarWidth: number;
  setSidebarWidth: (px: number) => void;
  /** 「Recent」's rows when there is room (`RECENT_ROWS`); remembered across launches. */
  recentRows: number;
  setRecentRows: (rows: number) => void;
  setSidebarLimits: (changes: Record<string, number>) => void;
  /** 「新建 Session」 (⌘N): pick the directory. */
  newSessionOpen: boolean;
  setNewSessionOpen: (open: boolean) => void;
  /** 「添加 Project」. */
  addProjectOpen: boolean;
  setAddProjectOpen: (open: boolean) => void;

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
  statsDrill: null,
  statsDrillRange: null,
  settingsOpen: false,
  collapsed: load(COLLAPSED_KEY, { sessions: false }),
  weeklyReset: load(WEEKLY_RESET_KEY, { weekday: 0, minutes: 0 }),
  autoConnect: load(AUTO_CONNECT_KEY, { on: true }).on !== false,
  setAutoConnect: (on) => {
    save(AUTO_CONNECT_KEY, { on });
    set({ autoConnect: on });
  },
  convWidth: clampWidth(load(CONV_WIDTH_KEY, { px: CONV_WIDTH.default }).px),
  sidebarExpanded: load(SIDEBAR_EXPANDED_KEY, {} as Record<string, boolean>),
  setSidebarExpanded: (changes) =>
    set((s) => {
      const sidebarExpanded = { ...s.sidebarExpanded, ...changes };
      save(SIDEBAR_EXPANDED_KEY, sidebarExpanded);
      return { sidebarExpanded };
    }),
  sidebarLimits: {},
  sidebarWidth: clamp(load(SIDEBAR_WIDTH_KEY, { px: SIDEBAR_WIDTH.default }).px, SIDEBAR_WIDTH),
  setSidebarWidth: (px) => {
    const sidebarWidth = clamp(px, SIDEBAR_WIDTH);
    save(SIDEBAR_WIDTH_KEY, { px: sidebarWidth });
    set({ sidebarWidth });
  },
  recentRows: clamp(load(RECENT_ROWS_KEY, { rows: RECENT_ROWS.default }).rows, RECENT_ROWS),
  setRecentRows: (rows) => {
    const recentRows = clamp(rows, RECENT_ROWS);
    save(RECENT_ROWS_KEY, { rows: recentRows });
    set({ recentRows });
  },
  setSidebarLimits: (changes) => set((s) => ({ sidebarLimits: { ...s.sidebarLimits, ...changes } })),
  newSessionOpen: false,
  setNewSessionOpen: (newSessionOpen) => set({ newSessionOpen }),
  addProjectOpen: false,
  setAddProjectOpen: (addProjectOpen) => set({ addProjectOpen }),
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
