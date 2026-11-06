import { useVirtualizer } from "@tanstack/react-virtual";
import { Folder, X } from "lucide-react";
import { useEffect, useMemo, useRef, type KeyboardEvent } from "react";
import { useConnect } from "@/features/connect";
import type { Drill, ProjectSummary, SessionSummary } from "@/ipc/bindings";
import { formatCount, formatSpan } from "@/lib/format";
import { useAllSessions, useProjects, useSessionList } from "@/queries";
import { useUi } from "@/state/ui";
import { buildSidebar, ROW_PX, type SideSession } from "./groups";
import { pendingSessions } from "./pending";
import { Row, SidebarColumn, SidebarDialogs, useSidebar, type Selection } from "./Sidebar";

/** Stable while the list loads, so the rows are not rebuilt on every render. */
const NO_SESSIONS: SessionSummary[] = [];

/**
 * The left column: 「Live」, the added Projects, 「Recent」; while a Project filter (from ⌘K) or a stats drill is on,
 * the matching Sessions take the Projects' place.
 */
export function SessionList() {
  const filtered = useUi((s) => s.projectIds.length > 0 || s.statsDrill != null);
  const data = useSidebar();
  return (
    <>
      <SidebarColumn data={data} middle={filtered ? (sel) => <FilteredList sel={sel} /> : undefined} />
      <SidebarDialogs data={data} />
    </>
  );
}

/**
 * Sessions in `ccrpc ls` that are not indexed yet (opened in clay, no prompt sent): placeholder rows at the top.
 * Once indexed they are ordinary rows with the same id.
 */
function usePending(list: SessionSummary[] | undefined, projects: ProjectSummary[] | undefined) {
  const ls = useConnect((s) => s.ls);
  const projectIds = useUi((s) => s.projectIds);
  const drill = useUi((s) => s.statsDrill);
  const wanted = !drill && ls.some((e) => e.session_id);
  // Every Session: which ones are indexed while the filter hides some.
  const { data: all } = useAllSessions(wanted);
  return useMemo(() => {
    if (!wanted || !list) return [];
    const indexed = all ? new Set(all.map((s) => s.id)) : null;
    return pendingSessions(ls, indexed, { projectIds, drill: drill != null }, projects ?? []);
  }, [wanted, list, all, ls, projectIds, drill, projects]);
}

/**
 * The Sessions of the ⌘K Project filter (started in or editing it) or of a stats drill, one line each like the
 * rest of the sidebar, latest user input first; a Session started in another Project is tagged with it.
 */
function FilteredList({ sel }: { sel: Selection }) {
  const { data } = useSessionList();
  const { data: projects } = useProjects();
  const projectIds = useUi((s) => s.projectIds);
  const drill = useUi((s) => s.statsDrill);
  const setDrill = useUi((s) => s.setStatsDrill);
  const setProjectIds = useUi((s) => s.setProjectIds);
  const pending = usePending(data, projects);

  const rows = useMemo<SideSession[]>(() => {
    // The same rows the sidebar builds (Live state, pending placeholders), as one list.
    const sb = buildSidebar(data ?? NO_SESSIONS, pending, projects ?? [], [], Date.now());
    const pend = sb.live.filter((s) => s.pending);
    const rest = [...sb.live.filter((s) => !s.pending), ...sb.recent].sort(
      (a, b) => b.timeMs - a.timeMs || a.id.localeCompare(b.id),
    );
    return [...pend, ...rest];
  }, [data, pending, projects]);
  const names = useMemo(() => new Map((projects ?? []).map((p) => [p.id, p.displayName])), [projects]);
  const tagOf = (s: SideSession) =>
    s.projectId && !projectIds.includes(s.projectId) ? (names.get(s.projectId) ?? null) : null;

  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_PX,
    getItemKey: (i) => rows[i]!.id,
    overscan: 10,
  });
  // A new filter starts at the top.
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [projectIds, drill]);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    e.preventDefault();
    const cur = rows.findIndex((s) => s.id === sel.sessionId);
    const next = Math.max(0, Math.min(rows.length - 1, cur < 0 ? 0 : cur + (e.key === "ArrowDown" ? 1 : -1)));
    const target = rows[next];
    if (!target) return;
    sel.pick(target.id);
    virtualizer.scrollToIndex(next, { align: "auto" });
  };

  return (
    <>
      <div
        data-tauri-drag-region="deep"
        className="flex h-[30px] shrink-0 items-center gap-1.5 border-b border-border pr-2 pl-4 text-[12px] text-secondary"
      >
        {projectIds.length > 0 && (
          <span
            data-testid="project-chip"
            className="flex h-5 min-w-0 items-center gap-1 rounded-[5px] bg-selection pr-0.5 pl-1.5 font-semibold text-accent"
          >
            <Folder size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
            <span data-testid="list-title" className="truncate">
              {projectIds.map((id) => names.get(id) ?? id).join("、")}
            </span>
            <button
              type="button"
              aria-label="清除 Project 筛选"
              onClick={() => setProjectIds([])}
              className="rounded-full p-0.5 hover:bg-black/10"
            >
              <X size={11} strokeWidth={2} aria-hidden />
            </button>
          </span>
        )}
        {drill && (
          <span
            data-testid="drill-chip"
            className="flex h-5 min-w-0 items-center gap-1 rounded-[5px] bg-selection pr-0.5 pl-1.5 font-semibold text-accent"
          >
            <span className="truncate">{drillLabel(drill)}</span>
            <button
              type="button"
              aria-label="清除筛选"
              onClick={() => setDrill(null)}
              className="rounded-full p-0.5 hover:bg-black/10"
            >
              <X size={11} strokeWidth={2} aria-hidden />
            </button>
          </span>
        )}
        <span data-testid="list-count" className="shrink-0 tabular-nums">
          {formatCount(rows.length)}
        </span>
      </div>
      <div
        ref={scrollRef}
        tabIndex={0}
        role="listbox"
        aria-label="Session 列表"
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-y-auto px-[6px] pt-[4px] pb-[10px] outline-none"
      >
        <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((v) => {
            const s = rows[v.index]!;
            return (
              <div
                key={v.key}
                className="absolute top-0 left-0 w-full"
                style={{ transform: `translateY(${v.start}px)` }}
              >
                <Row
                  s={s}
                  depth={0}
                  tag={tagOf(s)}
                  dotSlot
                  testId="session-row"
                  selected={s.id === sel.sessionId}
                  onOpen={sel.pick}
                />
              </div>
            );
          })}
        </div>
        {data && rows.length === 0 && <div className="p-6 text-center text-[12px] text-secondary">没有 Session</div>}
      </div>
    </>
  );
}

function drillLabel(d: Drill): string {
  switch (d.kind) {
    case "day": {
      const [, m, day] = d.day.split("-");
      return `${Number(m)}月${Number(day)}日`;
    }
    case "model":
      return `模型 ${d.model}`;
    case "span":
      return formatSpan(d.fromMs, d.toMs);
  }
}
