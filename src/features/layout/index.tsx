import { ChartColumn, List, Search } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useSyncExternalStore } from "react";
import { useHotkey } from "@/lib/hotkeys";
import { cn } from "@/lib/cn";
import { SIDEBAR_WIDTH, useUi, type CollapsiblePane } from "@/state/ui";

let fullScreen = false;
const fullScreenListeners = new Set<() => void>();
let watchingFullScreen = false;

/** Follows the window's full-screen state: asked of the native window in the app, guessed from the screen size in a browser. */
function watchFullScreen() {
  if (watchingFullScreen) return;
  watchingFullScreen = true;
  const update = (value: boolean) => {
    if (value === fullScreen) return;
    fullScreen = value;
    fullScreenListeners.forEach((l) => l());
  };
  if ("__TAURI_INTERNALS__" in window) {
    const win = getCurrentWindow();
    const check = () => void win.isFullscreen().then(update, () => {});
    check();
    void win.onResized(check);
  } else {
    const check = () => update(window.innerWidth === screen.width && window.innerHeight === screen.height);
    check();
    window.addEventListener("resize", check);
  }
}

function subscribeFullScreen(onChange: () => void) {
  watchFullScreen();
  fullScreenListeners.add(onChange);
  return () => void fullScreenListeners.delete(onChange);
}

/** A native full-screen window has no traffic lights; the toolbar takes their place. */
function useFullScreen(): boolean {
  return useSyncExternalStore(subscribeFullScreen, () => fullScreen);
}

/** The session list column is hidden by the user (the Subagent panel leaves it as it is). */
export function useSessionsHidden(): { hidden: boolean } {
  return { hidden: useUi((s) => s.collapsed.sessions) };
}

/**
 * Left padding for the main pane's header when the Session list is hidden, so it clears the traffic
 * lights and the toolbar; `undefined` otherwise. (The list has its own strip for them.)
 */
export function useLeadingInset(): string | undefined {
  const { hidden } = useSessionsHidden();
  const fullScreen = useFullScreen();
  if (!hidden) return undefined;
  // The toolbar holds four 28px buttons then (list, search, stats, settings).
  return fullScreen ? "pl-[148px]" : "pl-[212px]";
}

const TOOL_CLASS =
  "flex size-7 items-center justify-center rounded-md text-secondary outline-none hover:bg-text/8 focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-40 disabled:hover:bg-transparent aria-pressed:bg-selection aria-pressed:text-accent";

function Toggle({
  pane,
  label,
  keys,
  icon,
}: {
  pane: CollapsiblePane;
  label: string;
  keys: string;
  icon: React.ReactNode;
}) {
  const collapsed = useUi((s) => s.collapsed[pane]);
  const toggle = useUi((s) => s.toggleCollapsed);
  const action = `${collapsed ? "显示" : "隐藏"} ${label}`;
  return (
    <button
      type="button"
      aria-label={action}
      title={`${action}（${keys}）`}
      onClick={() => toggle(pane)}
      className={TOOL_CLASS}
    >
      {icon}
    </button>
  );
}

/** Opens the stats dialog. At the foot of the Session list, or in the toolbar while the list is hidden. */
export function StatsToggle({ className }: { className?: string }) {
  const active = useUi((s) => s.statsOpen);
  const setStatsOpen = useUi((s) => s.setStatsOpen);
  return (
    <button
      type="button"
      data-testid="nav-stats"
      aria-label="统计"
      title="统计"
      aria-pressed={active}
      onClick={() => setStatsOpen(!active)}
      className={cn(TOOL_CLASS, className)}
    >
      <ChartColumn size={15} strokeWidth={1.6} aria-hidden />
    </button>
  );
}

/**
 * Session list toggle and search, pinned right after the traffic lights like a native toolbar; `children` (stats
 * and settings while the list is hidden) follow them.
 */
export function Toolbar({ children }: { children?: React.ReactNode }) {
  const toggle = useUi((s) => s.toggleCollapsed);
  const fullScreen = useFullScreen();
  useHotkey("Ctrl+Meta+L", () => toggle("sessions"), { enableInInputs: true });

  return (
    <div
      data-testid="toolbar"
      className={cn("fixed top-[4px] z-10 flex gap-0.5", fullScreen ? "left-3" : "left-[78px]")}
    >
      <Toggle pane="sessions" label="Session 列表" keys="⌃⌘L" icon={<List size={16} strokeWidth={1.6} aria-hidden />} />
      <button
        type="button"
        data-testid="search-button"
        aria-label="搜索"
        title="搜索（⌘K）"
        onClick={() => useUi.getState().setSearchOpen(true)}
        className={TOOL_CLASS}
      >
        <Search size={16} strokeWidth={1.6} aria-hidden />
      </button>
      {children}
    </div>
  );
}

/**
 * The sidebar's right edge: dragged, its width (`SIDEBAR_WIDTH`); double-click goes back to the default; ← / →
 * when focused. Inside the sidebar, clear of the conversation.
 */
export function SidebarResizer() {
  const setWidth = useUi((s) => s.setSidebarWidth);
  const drag = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const x0 = e.clientX;
    const w0 = useUi.getState().sidebarWidth;
    const move = (m: PointerEvent) => setWidth(w0 + m.clientX - x0);
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = "col-resize";
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
  };
  const keys = (e: React.KeyboardEvent) => {
    const d = e.key === "ArrowRight" ? 20 : e.key === "ArrowLeft" ? -20 : 0;
    if (!d) return;
    e.preventDefault();
    setWidth(useUi.getState().sidebarWidth + d);
  };
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="拖动调节侧栏宽度，双击恢复默认"
      title="拖动调节侧栏宽度，双击恢复默认"
      tabIndex={0}
      data-testid="sidebar-resize"
      onPointerDown={drag}
      onDoubleClick={() => setWidth(SIDEBAR_WIDTH.default)}
      onKeyDown={keys}
      className="group/s absolute inset-y-0 right-0 z-10 w-[5px] cursor-col-resize outline-none"
    >
      <span className="absolute inset-y-0 right-0 w-px bg-transparent transition-colors group-hover/s:bg-accent group-focus-visible/s:bg-accent" />
    </div>
  );
}
