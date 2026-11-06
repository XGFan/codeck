import { useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronRight, Ellipsis, GripVertical, Plus, SquarePen } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { LiveDot, type LiveDotState } from "@/components/LiveDot";
import { NewSessionError, useConnect, useUnread } from "@/features/connect";
import { api } from "@/ipc";
import type { ProjectSummary, SidebarProject } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCount, formatRelative } from "@/lib/format";
import { queryKeys, useAllSessions, useIndexStatus, useProjects, useSidebarProjects } from "@/queries";
import { useUi } from "@/state/ui";
import { AddProjectDialog } from "./AddProjectDialog";
import {
  buildSidebar,
  fitRows,
  flatten,
  FIRST_ROWS,
  isOpen,
  LIVE_CHROME,
  MORE_ROWS,
  moveEntry,
  RECENT_CHROME,
  RECENT_KEY,
  recentRoom,
  reveal,
  ROW_PX,
  type Group,
  type Item,
  type Sidebar,
  type SideSession,
} from "./groups";
import { NewSessionDialog } from "./NewSessionDialog";
import { pendingSessions } from "./pending";

const NO_PROJECTS: ProjectSummary[] = [];

/** The sidebar's lists from every Session, the `ccrpc ls` placeholders and the added Projects. */
export function useSidebar(): {
  sidebar: Sidebar | null;
  projects: ProjectSummary[];
  entries: SidebarProject[] | null | undefined;
  save: (entries: SidebarProject[]) => void;
} {
  const { data: sessions } = useAllSessions();
  const { data: projects } = useProjects();
  const { data: saved } = useSidebarProjects();
  // One group per path even in a hand-edited settings file: groups and entries are matched by position.
  const entries = useMemo(
    () => (saved ? saved.filter((e, i) => saved.findIndex((o) => o.path === e.path) === i) : saved),
    [saved],
  );
  const ls = useConnect((s) => s.ls);
  const qc = useQueryClient();
  const sidebar = useMemo(() => {
    // Without the Projects no Session would find its group for a moment.
    if (!sessions || !projects || entries === undefined) return null;
    const pending = pendingSessions(ls, new Set(sessions.map((s) => s.id)), { projectIds: [], drill: false }, projects);
    return buildSidebar(sessions, pending, projects, entries ?? [], Date.now());
  }, [sessions, projects, entries, ls]);
  const saves = useRef(0);
  const save = (next: SidebarProject[]) => {
    qc.setQueryData(queryKeys.sidebarProjects, next);
    // Only the latest save's reply counts; a failed one shows what is saved again.
    const n = ++saves.current;
    api.setSidebarProjects(next).then(
      (saved) => n === saves.current && qc.setQueryData(queryKeys.sidebarProjects, saved),
      () => void qc.invalidateQueries({ queryKey: queryKeys.sidebarProjects }),
    );
  };
  return { sidebar, projects: projects ?? NO_PROJECTS, entries, save };
}

export type SidebarData = ReturnType<typeof useSidebar>;

/** 「新建 Session」 and 「添加 Project」; the latter opens by itself until the first Projects are added. */
export function SidebarDialogs({ data }: { data: SidebarData }) {
  const { sidebar, projects, entries, save } = data;
  const setAddProjectOpen = useUi((s) => s.setAddProjectOpen);
  const phase = useIndexStatus().data?.phase;
  const scanning = phase === undefined || phase === "scanning";
  const asked = useRef(false);
  useEffect(() => {
    // After the first scan: its suggestions need every Project (a parent folder's children included).
    if (entries === null && projects.length > 0 && !scanning && !asked.current) {
      asked.current = true;
      setAddProjectOpen(true);
    }
  }, [entries, projects, scanning, setAddProjectOpen]);
  return (
    <>
      <NewSessionDialog sidebar={sidebar} projects={projects} />
      {entries !== undefined && <AddProjectDialog projects={projects} entries={entries} onSave={save} />}
    </>
  );
}

/**
 * Selecting a Session from the sidebar (`pick`) or from elsewhere (⌘K, a link, 「+」): only the latter is brought
 * into view in the Project list (`pending` holds it until its row exists — a new Session shows up only once
 * `ccrpc ls` or the index has it).
 */
function useSelection() {
  const sessionId = useUi((s) => s.sessionId);
  const selectSession = useUi((s) => s.selectSession);
  const pending = useRef<string | null>(null);
  const pickedHere = useRef<string | null>(null);
  // Noted while rendering, not in an effect: the Project list's effects (a child's) run before this one's would.
  const last = useRef<string | null>(null);
  if (sessionId !== last.current) {
    last.current = sessionId;
    pending.current = sessionId && pickedHere.current !== sessionId ? sessionId : null;
    pickedHere.current = null;
  }
  const pick = (id: string) => {
    pickedHere.current = id;
    selectSession(id);
  };
  return { sessionId, pick, pending };
}

export type Selection = ReturnType<typeof useSelection>;

/** The column's height, followed as the window resizes. */
function useHeight(ref: React.RefObject<HTMLElement | null>): number {
  const [h, setH] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setH(el.clientHeight);
    const ro = new ResizeObserver(() => setH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return h;
}

/**
 * The sidebar: 「新建 Session」, then 「Live」 pinned at the top, the Project list (or `middle`, a filtered list)
 * scrolling in between, 「Recent」 pinned at the bottom.
 */
export function SidebarColumn({ data, middle }: { data: SidebarData; middle?: (sel: Selection) => ReactNode }) {
  const { sidebar, projects } = data;
  const setNewSessionOpen = useUi((s) => s.setNewSessionOpen);
  const expanded = useUi((s) => s.sidebarExpanded);
  const setExpanded = useUi((s) => s.setSidebarExpanded);
  const sel = useSelection();
  const names = useMemo(() => new Map(projects.map((p) => [p.id, p.displayName])), [projects]);
  const bodyRef = useRef<HTMLDivElement>(null);
  const height = useHeight(bodyRef);
  const live = sidebar?.live ?? [];
  const recent = sidebar?.recent ?? [];
  const recentOpen = isOpen(expanded, RECENT_KEY);
  const recentRows = useUi((s) => s.recentRows);
  const setRecentRows = useUi((s) => s.setRecentRows);
  const rows = fitRows(height, live.length, recent.length, recentOpen, recentRows);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* The toolbar (list toggle, search) floats in this strip, right after the traffic lights. */}
      <div data-tauri-drag-region="deep" className="h-10 shrink-0" />
      <div className="shrink-0 px-[6px] pb-[2px]">
        <button
          type="button"
          data-testid="new-session"
          onClick={() => setNewSessionOpen(true)}
          className={cn(ROW, "h-[30px] gap-[8px] pl-[10px]")}
        >
          <SquarePen size={15} strokeWidth={1.6} className="shrink-0 text-secondary" aria-hidden />
          <span className="flex-1">新建 Session</span>
          <span className="text-[11px] text-secondary">⌘N</span>
        </button>
      </div>
      <NewSessionError />
      <div ref={bodyRef} className="flex min-h-0 flex-1 flex-col">
        {live.length > 0 && <LivePanel sessions={live} rows={rows.live} names={names} sel={sel} />}
        <div className={cn("flex min-h-0 flex-1 flex-col", live.length > 0 && "border-t border-border")}>
          {middle ? middle(sel) : <ProjectList data={data} sel={sel} />}
        </div>
        {recent.length > 0 && (
          <RecentPanel
            sessions={recent}
            rows={rows.recent}
            room={recentRoom(height, live.length ? rows.live : 0)}
            onResize={setRecentRows}
            open={recentOpen}
            onToggle={() => setExpanded({ [RECENT_KEY]: !recentOpen })}
            names={names}
            sel={sel}
          />
        )}
      </div>
    </div>
  );
}

/** ↑↓ on a list (not on a button or a menu inside it, whose keys bubble here too) moves the selection in it. */
function arrowKeys(ids: string[], sel: Selection, onMoved?: (index: number) => void) {
  return (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    e.preventDefault();
    const cur = ids.indexOf(sel.sessionId ?? "");
    const next = Math.max(0, Math.min(ids.length - 1, cur < 0 ? 0 : cur + (e.key === "ArrowDown" ? 1 : -1)));
    const id = ids[next];
    if (!id) return;
    sel.pick(id);
    onMoved?.(next);
  };
}

/** 「Live」: every Live Session, as tall as its rows up to `rows`, then scrolling. */
function LivePanel({
  sessions,
  rows,
  names,
  sel,
}: {
  sessions: SideSession[];
  rows: number;
  names: Map<string, string>;
  sel: Selection;
}) {
  const listEl = useRef<HTMLDivElement>(null);
  const ids = sessions.map((s) => s.id);
  return (
    <section
      aria-label="Live"
      data-testid="live"
      className="flex shrink-0 flex-col pb-[6px]"
      style={{ height: LIVE_CHROME + rows * ROW_PX }}
    >
      <div className={SECTION}>
        Live<span className="font-normal tabular-nums">{sessions.length}</span>
      </div>
      <div
        tabIndex={0}
        role="listbox"
        aria-label="Live Session"
        onKeyDown={arrowKeys(ids, sel, (i) =>
          (listEl.current?.children[i] as HTMLElement | undefined)?.scrollIntoView({ block: "nearest" }),
        )}
        ref={listEl}
        className="min-h-0 flex-1 overflow-y-auto px-[6px] outline-none"
      >
        {sessions.map((s) => (
          <Row
            key={s.id}
            s={s}
            depth={0}
            tag={s.pending ? null : s.projectId ? (names.get(s.projectId) ?? null) : null}
            plainTag
            time={false}
            testId="live-row"
            selected={s.id === sel.sessionId}
            onOpen={sel.pick}
          />
        ))}
      </div>
    </section>
  );
}

/** 「Recent」: every Session that is not Live, latest user input first; `rows` tall, or its heading alone. */
function RecentPanel({
  sessions,
  rows,
  room,
  onResize,
  open,
  onToggle,
  names,
  sel,
}: {
  sessions: SideSession[];
  rows: number;
  /** The most rows there is room for. */
  room: number;
  /** Dragged (or ↑↓ on) its top edge: the rows it is to have. */
  onResize: (rows: number) => void;
  open: boolean;
  onToggle: () => void;
  names: Map<string, string>;
  sel: Selection;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: open ? sessions.length : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_PX,
    getItemKey: (i) => sessions[i]!.id,
    overscan: 8,
  });
  const ids = sessions.map((s) => s.id);
  return (
    <section
      aria-label="Recent"
      data-testid="recent"
      className="relative flex shrink-0 flex-col border-t border-border"
      // Collapsed: the heading and the border, without the list's bottom padding.
      style={{ height: open ? RECENT_CHROME + rows * ROW_PX : RECENT_CHROME - 4 }}
    >
      {open && <RowsHandle rows={rows} room={room} onResize={onResize} />}
      <button
        type="button"
        data-testid="recent-toggle"
        aria-expanded={open}
        onClick={onToggle}
        className={cn(
          SECTION,
          "h-[32px] w-full cursor-default text-left outline-none focus-visible:outline-2 focus-visible:outline-accent",
        )}
      >
        <Chevron open={open} />
        Recent<span className="font-normal tabular-nums">{formatCount(sessions.length)}</span>
      </button>
      {open && (
        <div
          ref={scrollRef}
          tabIndex={0}
          role="listbox"
          aria-label="Recent Session"
          onKeyDown={arrowKeys(ids, sel, (i) => virtualizer.scrollToIndex(i, { align: "auto" }))}
          className="min-h-0 flex-1 overflow-y-auto px-[6px] pb-[4px] outline-none"
        >
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((v) => {
              const s = sessions[v.index]!;
              return (
                <div
                  key={v.key}
                  className="absolute top-0 left-0 w-full"
                  style={{ transform: `translateY(${v.start}px)` }}
                >
                  <Row
                    s={s}
                    depth={0}
                    tag={s.projectId ? (names.get(s.projectId) ?? null) : null}
                    plainTag
                    testId="recent-row"
                    selected={s.id === sel.sessionId}
                    onOpen={sel.pick}
                  />
                </div>
              );
            })}
          </div>
        </div>
      )}
    </section>
  );
}

/**
 * 「Recent」's top edge: dragged, its height in whole rows (at least one, at most `room`); ↑ / ↓ when focused.
 * Inside the panel, clear of the Project list's last row.
 */
function RowsHandle({ rows, room, onResize }: { rows: number; room: number; onResize: (rows: number) => void }) {
  const fit = (n: number) => Math.max(1, Math.min(room, n));
  const drag = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const y0 = e.clientY;
    const move = (m: PointerEvent) => onResize(fit(rows + Math.round((y0 - m.clientY) / ROW_PX)));
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = "row-resize";
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
  };
  const keys = (e: React.KeyboardEvent) => {
    const d = e.key === "ArrowUp" ? 1 : e.key === "ArrowDown" ? -1 : 0;
    if (!d) return;
    e.preventDefault();
    onResize(fit(rows + d));
  };
  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label="拖动调节 Recent 的高度"
      aria-valuenow={rows}
      aria-valuemin={1}
      aria-valuemax={room}
      title="拖动调节 Recent 的高度"
      tabIndex={0}
      data-testid="recent-resize"
      onPointerDown={drag}
      onKeyDown={keys}
      className="group/h absolute inset-x-0 -top-px z-10 h-[6px] cursor-row-resize outline-none"
    >
      <span className="absolute inset-x-0 top-0 h-px bg-transparent transition-colors group-hover/h:bg-accent group-focus-visible/h:bg-accent" />
    </div>
  );
}

const HEIGHT: Record<Item["kind"], number> = {
  section: 32,
  group: ROW_PX,
  row: ROW_PX,
  more: ROW_PX,
  sub: ROW_PX,
  subsRest: ROW_PX,
};

const LIVE_TITLE: Record<LiveDotState, string> = { busy: "工作中", waiting: "等待作答", idle: "空闲" };

interface Drag {
  from: number;
  /** Insertion point among the groups, 0…length. */
  to: number;
  /** The pointer, from the top of the folded list. */
  y: number;
  /** Room above the folded list, so the pressed header can stay where it was in view. */
  pad: number;
}

/**
 * The added Projects as groups, in the user's order (drag a group's header, or 「上移 / 下移」 in its menu); a
 * nesting group lists its sub-Projects as sub-groups.
 */
function ProjectList({ data, sel }: { data: SidebarData; sel: Selection }) {
  const { sidebar, entries, save } = data;
  const { sessionId, pick, pending } = sel;
  const expanded = useUi((s) => s.sidebarExpanded);
  const setExpanded = useUi((s) => s.setSidebarExpanded);
  const limits = useUi((s) => s.sidebarLimits);
  const setLimits = useUi((s) => s.setSidebarLimits);
  const setAddProjectOpen = useUi((s) => s.setAddProjectOpen);
  const newSession = useConnect((s) => s.newSession);
  const [drag, setDrag] = useState<Drag | null>(null);

  const dragging = drag != null;
  const items = useMemo(
    () => (sidebar ? flatten(sidebar, expanded, limits, dragging) : []),
    [sidebar, expanded, limits, dragging],
  );

  // A Session opened elsewhere gets its row: its group opened, enough rows shown.
  useEffect(() => {
    const id = pending.current;
    if (!id || !sidebar || id !== sessionId) return;
    const st = useUi.getState();
    const r = reveal(sidebar, id, st.sidebarExpanded, st.sidebarLimits);
    if (!r) return;
    if (r.open.length) setExpanded(Object.fromEntries(r.open.map((k) => [k, true])));
    if (Object.keys(r.limits).length) setLimits(r.limits);
  }, [sessionId, sidebar, setExpanded, setLimits, pending]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => HEIGHT[items[i]!.kind],
    getItemKey: (i) => items[i]!.key,
    overscan: 12,
  });
  // Then scrolled to, once (not again as the list regroups under it).
  useEffect(() => {
    const id = pending.current;
    if (!id || id !== sessionId) return;
    const i = items.findIndex((it) => it.kind === "row" && it.s.id === id);
    if (i < 0) return;
    pending.current = null;
    virtualizer.scrollToIndex(i, { align: "auto" });
  }, [items, sessionId, virtualizer, pending]);

  const rowIds = items.flatMap((it) => (it.kind === "row" ? [it.s.id] : []));
  const onKeyDown = arrowKeys(rowIds, sel, (n) => {
    const i = items.findIndex((it) => it.kind === "row" && it.s.id === rowIds[n]);
    if (i >= 0) virtualizer.scrollToIndex(i, { align: "auto" });
  });

  const list = entries ?? [];
  const setNest = (g: Group, nest: boolean) => save(list.map((e) => (e.path === g.path ? { ...e, nest } : e)));
  const remove = (g: Group) => save(list.filter((e) => e.path !== g.path));
  const move = (from: number, to: number) => save(moveEntry(list, from, to));

  // Dragging a header: past 4px of movement the list shows the headers alone; released, the group moves there.
  const dragged = useRef(false);
  /** The entries as now, for a drag that ends after they changed. */
  const listRef = useRef(list);
  listRef.current = list;
  /** Ends a drag in progress (unmounted mid-drag: the listeners go too). */
  const stopDrag = useRef<(() => void) | null>(null);
  useEffect(() => () => stopDrag.current?.(), []);
  /** Where the list was scrolled before a drag folded it, and where the folded list is to be scrolled. */
  const dragView = useRef<{ before: number; folded: number; anchored: boolean } | null>(null);
  // Folded to headers: scrolled (over `pad` of room above) so the pressed header stays under the pointer.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    const v = dragView.current;
    if (!el || !v || !drag || v.anchored) return;
    el.scrollTop = v.folded;
    v.anchored = true;
  }, [drag]);

  const startDrag = (index: number, e: React.PointerEvent) => {
    const el = scrollRef.current;
    if (e.button !== 0 || list.length < 2 || !el) return;
    const startY = e.clientY;
    const header = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const pressInHeader = e.clientY - header.top;
    let current: Drag | null = null;
    const onMove = (ev: PointerEvent) => {
      let pad = current?.pad ?? 0;
      let y: number;
      if (!current) {
        if (Math.abs(ev.clientY - startY) < 4) return;
        // The header's place in the folded list, against where it is in view now.
        const inView = header.top - el.getBoundingClientRect().top;
        const folded = HEIGHT.section + index * ROW_PX - inView;
        pad = Math.max(0, -folded);
        dragView.current = { before: el.scrollTop, folded: Math.max(0, folded), anchored: false };
        y = HEIGHT.section + index * ROW_PX + pressInHeader + (ev.clientY - startY);
      } else {
        y = ev.clientY - el.getBoundingClientRect().top + el.scrollTop - pad;
      }
      // Over a header's upper half: before it; its lower half: after it.
      const to = Math.max(0, Math.min(listRef.current.length, Math.round((y - HEIGHT.section) / ROW_PX)));
      current = { from: index, to, y, pad };
      dragged.current = true;
      setDrag(current);
    };
    const cleanup = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKey, true);
      stopDrag.current = null;
    };
    const end = (commit: boolean) => {
      cleanup();
      if (current && commit && current.to !== current.from && current.to !== current.from + 1)
        save(moveEntry(listRef.current, current.from, current.to));
      setDrag(null);
      // Back where the list was before the drag folded it.
      const v = dragView.current;
      dragView.current = null;
      if (v) requestAnimationFrame(() => (el.scrollTop = v.before));
      if (!current) return;
      // The click that ends a drag does not toggle the group — nor the release after an Esc.
      if (commit) setTimeout(() => (dragged.current = false), 0);
      else window.addEventListener("pointerup", () => setTimeout(() => (dragged.current = false), 0), { once: true });
    };
    const onUp = () => end(true);
    const onCancel = () => end(false);
    const onKey = (ev: globalThis.KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      // Only cancels the drag: not also the Subagent panel's or a dialog's Esc.
      ev.preventDefault();
      ev.stopImmediatePropagation();
      end(false);
    };
    stopDrag.current = cleanup;
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKey, true);
  };

  const toggle = (key: string, open: boolean) => setExpanded({ [key]: !open });

  const render = (it: Item) => {
    switch (it.kind) {
      case "section":
        return (
          <div className={cn(SECTION, "h-full pr-0.5")}>
            <span className="flex-1">Project</span>
            {drag ? (
              <span className="font-normal">拖到要放的位置</span>
            ) : (
              <button
                type="button"
                data-testid="add-project-button"
                aria-label="添加 Project"
                title="添加 Project"
                onClick={() => setAddProjectOpen(true)}
                className={ICON}
              >
                <Plus size={14} strokeWidth={1.8} aria-hidden />
              </button>
            )}
          </div>
        );
      case "group":
        return (
          <GroupHeader
            g={it.g}
            open={it.open}
            dimmed={drag?.from === it.index}
            first={it.index === 0}
            last={it.index === list.length - 1}
            onPointerDown={(e) => startDrag(it.index, e)}
            onToggle={() => !dragged.current && toggle(it.key, it.open)}
            onNew={() => void newSession(it.g.path)}
            onNest={(nest) => setNest(it.g, nest)}
            onMove={(delta) => move(it.index, delta < 0 ? it.index - 1 : it.index + 2)}
            onRemove={() => remove(it.g)}
          />
        );
      case "row":
        return (
          <Row
            s={it.s}
            depth={it.depth}
            tag={null}
            testId="session-row"
            selected={it.s.id === sessionId}
            onOpen={pick}
          />
        );
      case "more":
        return (
          <button
            type="button"
            data-testid="show-more"
            onClick={() => setLimits({ [it.list]: (limits[it.list] ?? FIRST_ROWS) + MORE_ROWS })}
            className={cn(ROW, it.depth === 2 ? "pl-[46px]" : "pl-[34px]", "text-[12px] text-secondary")}
          >
            显示更多（{formatCount(it.rest)}）
          </button>
        );
      case "sub":
        return (
          <button
            type="button"
            data-testid="sidebar-sub"
            aria-expanded={it.open}
            onClick={() => toggle(it.key, it.open)}
            className={cn(ROW, "pl-[22px] font-medium")}
          >
            <Chevron open={it.open} />
            <span className="min-w-0 flex-1 truncate">{it.sub.name}</span>
            {!it.open && it.sub.sessions.some((s) => s.live) && <LiveDot state={liveOf(it.sub.sessions)!} />}
            <span className="shrink-0 text-[11px] font-normal text-secondary">
              {formatCount(it.sub.sessions.length)} · {formatRelative(it.sub.lastMs)}
            </span>
          </button>
        );
      case "subsRest":
        return (
          <button
            type="button"
            data-testid="sidebar-subs-rest"
            aria-expanded={it.open}
            onClick={() => toggle(it.key, it.open)}
            className={cn(ROW, "pl-[22px] text-[12px] text-secondary")}
          >
            <Chevron open={it.open} />
            <span className="flex-1">其他 {formatCount(it.count)} 个子 Project</span>
          </button>
        );
    }
  };

  const lifted = drag && sidebar?.groups[drag.from];
  return (
    <div
      ref={scrollRef}
      tabIndex={0}
      role="listbox"
      aria-label="Session 列表"
      onKeyDown={onKeyDown}
      className={cn(
        "min-h-0 flex-1 overflow-y-auto px-[6px] pb-[10px] outline-none",
        drag && "cursor-grabbing select-none",
      )}
    >
      <div
        className="relative w-full"
        // While dragging, room above (`pad`) and below, so the folded list can sit with the pressed header in place.
        style={{ height: virtualizer.getTotalSize() + (drag ? drag.pad + (scrollRef.current?.clientHeight ?? 0) : 0) }}
      >
        {virtualizer.getVirtualItems().map((v) => {
          const it = items[v.index]!;
          return (
            <div
              key={v.key}
              className="absolute top-0 left-0 w-full"
              style={{ height: HEIGHT[it.kind], transform: `translateY(${v.start + (drag?.pad ?? 0)}px)` }}
            >
              {render(it)}
            </div>
          );
        })}
        {drag && (
          <div
            data-testid="drop-line"
            aria-hidden
            className="pointer-events-none absolute right-1 left-1 h-0.5 rounded-full bg-accent"
            style={{ top: drag.pad + HEIGHT.section + drag.to * ROW_PX - 1 }}
          />
        )}
        {lifted && (
          <div
            aria-hidden
            className="pointer-events-none absolute right-1 left-2 flex h-[30px] -rotate-1 items-center gap-1.5 rounded-[7px] border border-border bg-ground px-2 font-semibold shadow-[0_8px_24px_rgba(0,0,0,0.16),0_1px_3px_rgba(0,0,0,0.08)]"
            style={{ top: drag.pad + drag.y - 15 }}
          >
            <GripVertical size={12} strokeWidth={2} className="text-secondary" />
            {lifted.name}
            <span className="text-[11px] font-normal text-secondary">{formatCount(lifted.total)}</span>
          </div>
        )}
      </div>
      {sidebar && sidebar.groups.length === 0 && (
        <div className="px-2.5 py-1 text-[12px] text-secondary">点「+」添加 Project，它的 Session 会列在这里。</div>
      )}
    </div>
  );
}

export const ROW =
  "flex h-[28px] w-full cursor-default items-center gap-[7px] rounded-md pr-[8px] text-left text-[13px] text-text outline-none hover:bg-black/[0.04] focus-visible:outline-2 focus-visible:outline-accent dark:hover:bg-white/[0.05]";
const ICON =
  "flex size-[24px] shrink-0 items-center justify-center rounded-[5px] text-secondary outline-none hover:bg-text/8 focus-visible:outline-2 focus-visible:outline-accent";
export const SECTION =
  "flex h-[28px] shrink-0 items-center gap-[6px] pr-[6px] pl-[16px] text-[11px] font-semibold text-secondary";

function liveOf(sessions: SideSession[]): LiveDotState | null {
  const order: LiveDotState[] = ["waiting", "busy", "idle"];
  return order.find((l) => sessions.some((s) => s.live === l)) ?? null;
}

function Chevron({ open }: { open: boolean }) {
  const Icon = open ? ChevronDown : ChevronRight;
  return <Icon size={12} strokeWidth={2} className="shrink-0 text-secondary" aria-hidden />;
}

function UnreadDot({ id }: { id: string }) {
  const unread = useUnread(id);
  return unread ? (
    <span data-testid="unread" title="有新回复" className="size-1.5 shrink-0 rounded-full bg-accent" />
  ) : null;
}

function GroupHeader({
  g,
  open,
  dimmed,
  first,
  last,
  onPointerDown,
  onToggle,
  onNew,
  onNest,
  onMove,
  onRemove,
}: {
  g: Group;
  open: boolean;
  dimmed: boolean;
  first: boolean;
  last: boolean;
  onPointerDown: (e: React.PointerEvent) => void;
  onToggle: () => void;
  onNew: () => void;
  onNest: (nest: boolean) => void;
  onMove: (delta: -1 | 1) => void;
  onRemove: () => void;
}) {
  return (
    <div
      data-testid="sidebar-group"
      data-path={g.path}
      className={cn(
        "group/g flex h-full items-center rounded-md pr-[4px] hover:bg-black/[0.04] has-[[data-state=open]]:bg-black/[0.04] dark:hover:bg-white/[0.05]",
        dimmed && "opacity-35",
      )}
    >
      <button
        type="button"
        aria-expanded={open}
        onPointerDown={onPointerDown}
        onClick={onToggle}
        title={g.path}
        className="flex h-full min-w-0 flex-1 cursor-default items-center gap-[6px] rounded-md pr-[4px] pl-[10px] text-left font-semibold outline-none focus-visible:outline-2 focus-visible:outline-accent"
      >
        <span className="flex w-[12px] shrink-0 justify-center">
          <span className="group-hover/g:hidden">
            <Chevron open={open} />
          </span>
          <GripVertical
            size={12}
            strokeWidth={2}
            className="hidden shrink-0 text-secondary group-hover/g:block"
            aria-hidden
          />
        </span>
        <span className="truncate">{g.name}</span>
        <span className="shrink-0 text-[11px] font-normal text-secondary tabular-nums">{formatCount(g.total)}</span>
        {!open && g.live && <LiveDot state={g.live} title="有 Live Session" />}
        <span className="flex-1" />
        {!open && g.lastMs > 0 && (
          <span className="shrink-0 text-[11px] font-normal text-secondary group-hover/g:hidden group-has-[[data-state=open]]/g:hidden">
            {formatRelative(g.lastMs)}
          </span>
        )}
      </button>
      <span className="hidden items-center group-hover/g:flex group-focus-within/g:flex group-has-[[data-state=open]]/g:flex">
        <button
          type="button"
          aria-label={`在 ${g.name} 新建 Session`}
          title="新建 Session"
          onClick={onNew}
          className={ICON}
        >
          <Plus size={14} strokeWidth={1.8} aria-hidden />
        </button>
        <DropdownMenu>
          <DropdownMenuTrigger aria-label={`${g.name} 的更多操作`} className={ICON}>
            <Ellipsis size={14} strokeWidth={1.8} aria-hidden />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {(g.childProjects > 0 || g.nest) && (
              <>
                <DropdownMenuCheckboxItem checked={g.nest} onCheckedChange={(c) => onNest(c === true)}>
                  收拢子 Project（{formatCount(g.childProjects)} 个）
                </DropdownMenuCheckboxItem>
                <DropdownMenuSeparator />
              </>
            )}
            <DropdownMenuItem disabled={first} onSelect={() => onMove(-1)}>
              上移
            </DropdownMenuItem>
            <DropdownMenuItem disabled={last} onSelect={() => onMove(1)}>
              下移
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={onRemove}>从侧栏移除</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </span>
    </div>
  );
}

/**
 * One Session on one line: Live dot, title, 「等待作答」, unread, a tag (its Project in 「Live」 / 「Recent」 /
 * a filtered list, a pending Session's directory), the time of its latest user input.
 */
export function Row({
  s,
  depth,
  tag,
  plainTag = false,
  time = true,
  dotSlot = depth > 0,
  testId,
  selected,
  onOpen,
}: {
  s: SideSession;
  /** 0: a flat list; 1, 2: in a group, in a sub-group. */
  depth: 0 | 1 | 2;
  tag: string | null;
  /** The tag as plain grey text (a list where every row has one) instead of a chip. */
  plainTag?: boolean;
  /** Shows the time of the latest user input (not in 「Live」). */
  time?: boolean;
  /** Keeps the Live dot's room on rows without one, so titles line up in a list that mixes both. */
  dotSlot?: boolean;
  testId: string;
  selected: boolean;
  onOpen: (id: string) => void;
}) {
  const chip = s.pending ? (s.cwd?.split("/").filter(Boolean).pop() ?? null) : plainTag ? null : tag;
  return (
    <div
      role="option"
      aria-selected={selected}
      data-testid={testId}
      data-session-id={s.id}
      data-pending={s.pending ? "" : undefined}
      onClick={() => onOpen(s.id)}
      title={s.pending ? (s.cwd ?? undefined) : undefined}
      className={cn(
        ROW,
        depth === 2 ? "pl-[46px]" : depth === 1 ? "pl-[30px]" : "pl-[10px]",
        selected && "bg-selection hover:bg-selection",
      )}
    >
      {s.live ? (
        <LiveDot state={s.live} title={LIVE_TITLE[s.live]} testId="live-dot" />
      ) : (
        dotSlot && <span className="w-2 shrink-0" aria-hidden />
      )}
      <span className="min-w-0 flex-1 truncate">{s.title}</span>
      {s.live === "waiting" && (
        <span
          data-testid="waiting-tag"
          className="shrink-0 rounded border border-tag-border bg-tag px-1 text-[11px] leading-[15px] font-semibold text-tag-text"
        >
          等待作答
        </span>
      )}
      <UnreadDot id={s.id} />
      {chip && (
        <span className="max-w-[110px] shrink-0 truncate rounded bg-text/8 px-[5px] text-[11px] text-secondary">
          {chip}
        </span>
      )}
      {plainTag && !s.pending && tag && (
        <span className="max-w-[88px] shrink-0 truncate text-[11px] text-secondary">{tag}</span>
      )}
      {time && !s.pending && <span className="shrink-0 text-[11px] text-secondary">{formatRelative(s.timeMs)}</span>}
    </div>
  );
}
