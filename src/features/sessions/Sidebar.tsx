import { useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronRight, Ellipsis, Plus, SquarePen } from "lucide-react";
import { useEffect, useMemo, useRef, type KeyboardEvent } from "react";
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
  flatten,
  FIRST_ROWS,
  MORE_ROWS,
  reveal,
  type Group,
  type Item,
  type Sidebar,
  type SideSession,
} from "./groups";
import { NewSessionDialog } from "./NewSessionDialog";
import { pendingSessions } from "./pending";

const NO_PROJECTS: ProjectSummary[] = [];

/** The sidebar's groups from every Session, the `ccrpc ls` placeholders and the added Projects. */
export function useSidebar(): {
  sidebar: Sidebar | null;
  projects: ProjectSummary[];
  entries: SidebarProject[] | null | undefined;
  save: (entries: SidebarProject[]) => void;
} {
  const { data: sessions } = useAllSessions();
  const { data: projects } = useProjects();
  const { data: entries } = useSidebarProjects();
  const ls = useConnect((s) => s.ls);
  const qc = useQueryClient();
  const sidebar = useMemo(() => {
    // Without the Projects every Session would land in 「其他」 for a moment.
    if (!sessions || !projects || entries === undefined) return null;
    const pending = pendingSessions(
      ls,
      new Set(sessions.map((s) => s.id)),
      { projectIds: [], drill: false },
      projects ?? [],
    );
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

/** 「新建 Session」 and 「添加 Project」; the latter opens by itself until the first Projects are added. */
export function SidebarDialogs({ data }: { data: ReturnType<typeof useSidebar> }) {
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

const HEIGHT: Record<Item["kind"], number> = {
  section: 38,
  running: 28,
  group: 28,
  row: 28,
  more: 28,
  sub: 28,
  subsRest: 28,
  other: 40,
};

const LIVE_TITLE: Record<LiveDotState, string> = { busy: "工作中", waiting: "等待作答", idle: "空闲" };

/**
 * The sidebar: 「新建 Session」, the Live Sessions under 「运行中」, then the added Projects as groups (latest user
 * input first; a nesting group lists its sub-Projects as sub-groups), then 「其他」.
 */
export function GroupedList({ data }: { data: ReturnType<typeof useSidebar> }) {
  const { sidebar, projects, entries, save } = data;
  const sessionId = useUi((s) => s.sessionId);
  const selectSession = useUi((s) => s.selectSession);
  const expanded = useUi((s) => s.sidebarExpanded);
  const setExpanded = useUi((s) => s.setSidebarExpanded);
  const limits = useUi((s) => s.sidebarLimits);
  const setLimits = useUi((s) => s.setSidebarLimits);
  const setNewSessionOpen = useUi((s) => s.setNewSessionOpen);
  const setAddProjectOpen = useUi((s) => s.setAddProjectOpen);
  const newSession = useConnect((s) => s.newSession);

  const names = useMemo(() => new Map(projects.map((p) => [p.id, p.displayName])), [projects]);
  const items = useMemo(
    () => (sidebar ? flatten(sidebar, expanded, limits, names) : []),
    [sidebar, expanded, limits, names],
  );

  /**
   * The selection still to bring into view: set when it changes (unless it was picked in the sidebar itself), kept
   * until its row exists — a new Session shows up only once `ccrpc ls` or the index has it.
   */
  const pendingReveal = useRef<string | null>(null);
  const pickedHere = useRef<string | null>(null);
  useEffect(() => {
    pendingReveal.current = sessionId && pickedHere.current !== sessionId ? sessionId : null;
    pickedHere.current = null;
  }, [sessionId]);
  // A Session opened elsewhere (⌘K, a link, 「+」) gets its row: its group opened, enough rows shown.
  useEffect(() => {
    const id = pendingReveal.current;
    if (!id || !sidebar || id !== sessionId) return;
    const st = useUi.getState();
    const r = reveal(sidebar, id, st.sidebarExpanded, st.sidebarLimits);
    if (!r) return;
    if (r.open.length) setExpanded(Object.fromEntries(r.open.map((k) => [k, true])));
    if (Object.keys(r.limits).length) setLimits(r.limits);
  }, [sessionId, sidebar, setExpanded, setLimits]);
  const pick = (id: string) => {
    pickedHere.current = id;
    selectSession(id);
  };

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
    const id = pendingReveal.current;
    if (!id || id !== sessionId) return;
    const i = items.findIndex((it) => it.kind === "row" && it.s.id === id);
    if (i < 0) return;
    pendingReveal.current = null;
    virtualizer.scrollToIndex(i, { align: "auto" });
  }, [items, sessionId, virtualizer]);

  const toggle = (key: string, open: boolean) => setExpanded({ [key]: !open });
  const onKeyDown = (e: KeyboardEvent) => {
    // Only on the list itself: not in a group's menu (portaled, its keys still bubble here) or on a button.
    if (e.target !== e.currentTarget || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    e.preventDefault();
    const rows = items.flatMap((it, i) => (it.kind === "row" ? [{ id: it.s.id, i }] : []));
    const cur = rows.findIndex((r) => r.id === sessionId);
    const next = rows[Math.max(0, Math.min(rows.length - 1, cur < 0 ? 0 : cur + (e.key === "ArrowDown" ? 1 : -1)))];
    if (!next) return;
    pick(next.id);
    virtualizer.scrollToIndex(next.i, { align: "auto" });
  };

  const setNest = (g: Group, nest: boolean) =>
    save((entries ?? []).map((e) => (e.path === g.path ? { ...e, nest } : e)));
  const remove = (g: Group) => save((entries ?? []).filter((e) => e.path !== g.path));

  const render = (it: Item) => {
    switch (it.kind) {
      case "section":
        return (
          <div className="flex h-full items-end pb-1 pl-4 pr-2 text-[11px] font-semibold text-secondary">
            <span className="flex h-6 flex-1 items-center gap-1.5">
              {it.label}
              {it.count != null && <span className="font-normal tabular-nums">{it.count}</span>}
            </span>
            {it.key === "sec:projects" && (
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
      case "running":
        return (
          <button
            type="button"
            data-testid="running-row"
            data-session-id={it.s.id}
            aria-current={it.s.id === sessionId || undefined}
            onClick={() => pick(it.s.id)}
            className={cn(ROW, "pl-4", it.s.id === sessionId && "bg-selection")}
          >
            <LiveDot state={it.s.live!} title={LIVE_TITLE[it.s.live!]} />
            <span className="min-w-0 flex-1 truncate">{it.s.title}</span>
            <UnreadDot id={it.s.id} />
            {it.project && (
              <span className="max-w-[96px] shrink-0 truncate text-[11px] text-secondary">{it.project}</span>
            )}
          </button>
        );
      case "group":
        return (
          <GroupHeader
            g={it.g}
            open={it.open}
            onToggle={() => toggle(it.key, it.open)}
            onNew={() => void newSession(it.g.path)}
            onNest={(nest) => setNest(it.g, nest)}
            onRemove={() => remove(it.g)}
          />
        );
      case "row":
        return <Row s={it.s} depth={it.depth} tag={it.tag} selected={it.s.id === sessionId} onOpen={pick} />;
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
      case "other":
        return (
          <div className="mt-2.5 h-[30px] border-t border-border pt-1">
            <button
              type="button"
              data-testid="sidebar-other"
              aria-expanded={it.open}
              onClick={() => toggle(it.key, it.open)}
              className={cn(ROW, "pl-2.5 font-semibold")}
            >
              <Chevron open={it.open} />
              <span className="text-secondary">其他</span>
              <span className="text-[11px] font-normal text-secondary tabular-nums">{formatCount(it.count)}</span>
              <span className="flex-1" />
              <span className="text-[11px] font-normal text-secondary">未添加的 Project</span>
            </button>
          </div>
        );
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* The toolbar (list toggle, search) floats in this strip, right after the traffic lights. */}
      <div data-tauri-drag-region="deep" className="h-10 shrink-0" />
      <div className="shrink-0 px-1.5 pb-0.5">
        <button
          type="button"
          data-testid="new-session"
          onClick={() => setNewSessionOpen(true)}
          className={cn(ROW, "h-[30px] gap-2 pl-2.5")}
        >
          <SquarePen size={15} strokeWidth={1.6} className="shrink-0 text-secondary" aria-hidden />
          <span className="flex-1">新建 Session</span>
          <span className="text-[11px] text-secondary">⌘N</span>
        </button>
      </div>
      <NewSessionError />
      <div
        ref={scrollRef}
        tabIndex={0}
        role="listbox"
        aria-label="Session 列表"
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2.5 outline-none"
      >
        <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((v) => {
            const it = items[v.index]!;
            return (
              <div
                key={v.key}
                className="absolute top-0 left-0 w-full"
                style={{ height: HEIGHT[it.kind], transform: `translateY(${v.start}px)` }}
              >
                {render(it)}
              </div>
            );
          })}
        </div>
        {sidebar && sidebar.groups.length === 0 && (
          <div className="px-4 py-2 text-[12px] text-secondary">点「+」添加 Project，它的 Session 会列在这里。</div>
        )}
      </div>
    </div>
  );
}

const ROW =
  "flex h-7 w-full cursor-default items-center gap-[7px] rounded-md pr-2 text-left text-[13px] text-text outline-none hover:bg-black/[0.04] focus-visible:outline-2 focus-visible:outline-accent dark:hover:bg-white/[0.05]";
const ICON =
  "flex size-6 shrink-0 items-center justify-center rounded-[5px] text-secondary outline-none hover:bg-text/8 focus-visible:outline-2 focus-visible:outline-accent";

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
  onToggle,
  onNew,
  onNest,
  onRemove,
}: {
  g: Group;
  open: boolean;
  onToggle: () => void;
  onNew: () => void;
  onNest: (nest: boolean) => void;
  onRemove: () => void;
}) {
  return (
    <div
      data-testid="sidebar-group"
      data-path={g.path}
      className="group/g flex h-full items-center rounded-md pr-1 hover:bg-black/[0.04] has-[[data-state=open]]:bg-black/[0.04] dark:hover:bg-white/[0.05]"
    >
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        title={g.path}
        className="flex h-full min-w-0 flex-1 cursor-default items-center gap-1.5 rounded-md pr-1 pl-2.5 text-left font-semibold outline-none focus-visible:outline-2 focus-visible:outline-accent"
      >
        <Chevron open={open} />
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
            <DropdownMenuItem onSelect={onRemove}>从侧栏移除</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </span>
    </div>
  );
}

function Row({
  s,
  depth,
  tag,
  selected,
  onOpen,
}: {
  s: SideSession;
  depth: 1 | 2;
  tag: string | null;
  selected: boolean;
  onOpen: (id: string) => void;
}) {
  const label = s.pending ? (s.cwd?.split("/").filter(Boolean).pop() ?? null) : tag;
  return (
    <div
      role="option"
      aria-selected={selected}
      data-testid="session-row"
      data-session-id={s.id}
      data-pending={s.pending ? "" : undefined}
      onClick={() => onOpen(s.id)}
      title={s.pending ? (s.cwd ?? undefined) : undefined}
      className={cn(ROW, depth === 2 ? "pl-[46px]" : "pl-[30px]", selected && "bg-selection hover:bg-selection")}
    >
      {s.live ? (
        <LiveDot state={s.live} title={LIVE_TITLE[s.live]} testId="live-dot" />
      ) : (
        <span className="w-2 shrink-0" aria-hidden />
      )}
      <span className="min-w-0 flex-1 truncate">{s.title}</span>
      <UnreadDot id={s.id} />
      {label && (
        <span className="max-w-[110px] shrink-0 truncate rounded bg-text/8 px-[5px] text-[11px] text-secondary">
          {label}
        </span>
      )}
      {!s.pending && <span className="shrink-0 text-[11px] text-secondary">{formatRelative(s.timeMs)}</span>}
    </div>
  );
}
