import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, Folder, GitBranch, X } from "lucide-react";
import { useMemo, useRef, type KeyboardEvent } from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { NewSessionButton, NewSessionError, useConnect, useUnread } from "@/features/connect";
import type { Drill, ProjectSummary, SessionQuery, SessionSort, SessionSummary } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCost, formatRelative, formatSpan, formatTokens } from "@/lib/format";
import { useProjects, useSessionList, useSessions } from "@/queries";
import { useUi } from "@/state/ui";
import { pendingSessions, type PendingSession } from "./pending";

const ROW_HEIGHT = 60;
/** Stable while the list loads, so the rows are not rebuilt on every render. */
const NO_SESSIONS: SessionSummary[] = [];
const nf = new Intl.NumberFormat("en-US");

const SORTS: { value: SessionSort; menu: string; order: string }[] = [
  { value: "lastActive", menu: "时间", order: "时间" },
  { value: "created", menu: "创建", order: "创建时间" },
  { value: "messages", menu: "消息数", order: "消息数" },
  { value: "tokens", menu: "Token", order: "Token" },
  { value: "cost", menu: "Cost", order: "Cost" },
];

/** Every Session, unfiltered: which ones are indexed while a Project filter hides some. */
const ALL_SESSIONS: SessionQuery = {
  projectIds: [],
  sort: "lastActive",
  descending: true,
  timeRange: null,
  drill: null,
};

/**
 * Sessions in `ccrpc ls` that are not indexed yet (opened in clay, no prompt sent): placeholder rows at the top.
 * Once indexed they are ordinary rows with the same id.
 */
function usePending(list: SessionSummary[] | undefined, projects: ProjectSummary[] | undefined): PendingSession[] {
  const ls = useConnect((s) => s.ls);
  const projectIds = useUi((s) => s.projectIds);
  const drill = useUi((s) => s.statsDrill);
  const filtered = projectIds.length > 0;
  const wanted = !drill && ls.some((e) => e.session_id);
  const { data: all } = useSessions(ALL_SESSIONS, filtered && wanted);
  const source = filtered ? all : list;
  return useMemo(() => {
    if (!wanted) return [];
    const indexed = source ? new Set(source.map((s) => s.id)) : null;
    return pendingSessions(ls, indexed, { projectIds, drill: drill != null }, projects ?? []);
  }, [wanted, source, ls, projectIds, drill, projects]);
}

type Item = { kind: "pending"; p: PendingSession } | { kind: "session"; s: SessionSummary };

export function SessionList() {
  const { data } = useSessionList();
  const { data: projects } = useProjects();
  const projectIds = useUi((s) => s.projectIds);
  const sessionId = useUi((s) => s.sessionId);
  const selectSession = useUi((s) => s.selectSession);
  const sort = useUi((s) => s.sessionSort);
  const descending = useUi((s) => s.sessionDescending);
  const setSort = useUi((s) => s.setSessionSort);
  const drill = useUi((s) => s.statsDrill);
  const setDrill = useUi((s) => s.setStatsDrill);
  const setProjectIds = useUi((s) => s.setProjectIds);

  const pending = usePending(data, projects);
  const list = data ?? NO_SESSIONS;
  const items = useMemo<Item[]>(
    () => [
      ...pending.map((p) => ({ kind: "pending" as const, p })),
      ...list.map((s) => ({ kind: "session" as const, s })),
    ],
    [pending, list],
  );
  const itemId = (i: Item) => (i.kind === "pending" ? i.p.id : i.s.id);
  const names = useMemo(() => new Map((projects ?? []).map((p: ProjectSummary) => [p.id, p.displayName])), [projects]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });

  const sortInfo = SORTS.find((s) => s.value === sort)!;

  const open = (id: string) => {
    selectSession(id);
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const cur = items.findIndex((i) => itemId(i) === sessionId);
    const next = Math.max(0, Math.min(items.length - 1, cur < 0 ? 0 : cur + (e.key === "ArrowDown" ? 1 : -1)));
    const target = items[next];
    if (!target) return;
    open(itemId(target));
    virtualizer.scrollToIndex(next, { align: "auto" });
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* The toolbar (list toggle, search) floats in this strip, right after the traffic lights. */}
      <div data-tauri-drag-region="deep" className="h-10 shrink-0" />
      <div
        data-tauri-drag-region="deep"
        className="flex h-[30px] shrink-0 items-center gap-1.5 border-b border-border pr-2 pl-4 text-[12px] text-secondary"
      >
        {projectIds.length > 0 ? (
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
        ) : (
          <span data-testid="list-title" className="font-semibold text-text">
            全部 Session
          </span>
        )}
        <span data-testid="list-count" className="shrink-0 tabular-nums">
          {nf.format(items.length)}
        </span>
        <span className="flex-1" />
        <NewSessionButton />
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label="排序方式"
            className="flex h-[22px] shrink-0 items-center gap-0.5 rounded-md px-1.5 text-[12px] text-secondary outline-none hover:bg-text/8 focus-visible:outline-2 focus-visible:outline-accent"
          >
            按{sortInfo.menu}
            {descending ? (
              <ArrowDown size={11} strokeWidth={2} aria-hidden />
            ) : (
              <ArrowUp size={11} strokeWidth={2} aria-hidden />
            )}
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {SORTS.map((s) => (
              <DropdownMenuItem key={s.value} onSelect={() => setSort(s.value)}>
                <span className="w-3">{s.value === sort ? "✓" : ""}</span>
                {s.menu}
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => setSort(sort, !descending)}>
              <span className="w-3" />
              {descending ? "改为正序" : "改为倒序"}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <NewSessionError />
      {drill && (
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-3.5 py-1.5 text-[12px]">
          <span className="text-secondary">筛选</span>
          <FilterChip testId="drill-chip" label="清除筛选" onClear={() => setDrill(null)}>
            <span className="truncate">{drillLabel(drill)}</span>
          </FilterChip>
        </div>
      )}
      <div
        ref={scrollRef}
        tabIndex={0}
        role="listbox"
        aria-label="Session 列表"
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-y-auto outline-none"
      >
        <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((v) => {
            const item = items[v.index]!;
            const id = itemId(item);
            return (
              <div
                key={id}
                className="absolute top-0 left-0 w-full"
                style={{ height: ROW_HEIGHT, transform: `translateY(${v.start}px)` }}
              >
                {item.kind === "pending" ? (
                  <PendingRow p={item.p} selected={id === sessionId} onOpen={open} />
                ) : (
                  <SessionRow s={item.s} selected={id === sessionId} byCreated={sort === "created"} onOpen={open} />
                )}
              </div>
            );
          })}
        </div>
        {data && items.length === 0 && <div className="p-6 text-center text-[12px] text-secondary">没有 Session</div>}
      </div>
    </div>
  );
}

function FilterChip({
  testId,
  label,
  onClear,
  children,
}: {
  testId: string;
  label: string;
  onClear: () => void;
  children: React.ReactNode;
}) {
  return (
    <span
      data-testid={testId}
      className="flex min-w-0 items-center gap-1 rounded-full bg-selection px-2 py-0.5 text-text"
    >
      {children}
      <button type="button" aria-label={label} onClick={onClear} className="rounded-full p-0.5 hover:bg-black/10">
        <X size={11} strokeWidth={2} aria-hidden />
      </button>
    </span>
  );
}

function SessionRow({
  s,
  selected,
  byCreated,
  onOpen,
}: {
  s: SessionSummary;
  selected: boolean;

  byCreated: boolean;
  onOpen: (id: string) => void;
}) {
  const state = s.live?.status === "busy" ? "busy" : s.live?.status === "waiting" ? "waiting" : "idle";
  const unread = useUnread(s.id);
  return (
    <div
      role="option"
      aria-selected={selected}
      data-testid="session-row"
      data-session-id={s.id}
      onClick={() => onOpen(s.id)}
      className={rowClass(selected)}
    >
      <div className="flex items-center gap-1.5">
        {s.live && <LiveDot state={state} />}
        <span className="min-w-0 flex-1 truncate font-semibold">{s.title}</span>
        {unread && <span data-testid="unread" title="有新回复" className="size-1.5 shrink-0 rounded-full bg-accent" />}
        <span className="shrink-0 text-[11px] text-secondary">
          {formatRelative(byCreated ? s.createdMs : s.lastActiveMs)}
        </span>
      </div>
      <div className="flex items-center gap-2.5 text-[11px] text-secondary">
        {s.gitBranch && (
          <span className="flex min-w-0 items-center gap-[3px]">
            <GitBranch size={11} strokeWidth={1.5} className="shrink-0" aria-hidden />
            <span className="truncate">{s.gitBranch}</span>
          </span>
        )}
        <span className="flex-1" />
        <span className="shrink-0">{s.messageCount} 条</span>
        <span className="shrink-0">{formatTokens(s.tokens.output)}</span>
        <span data-testid="row-cost" className="shrink-0">
          {formatCost(s.cost)}
        </span>
      </div>
    </div>
  );
}

const rowClass = (selected: boolean) =>
  cn(
    "flex h-full cursor-default flex-col justify-center gap-[3px] border-b border-border/70 px-3.5",
    selected
      ? "bg-selection shadow-[inset_3px_0_0_var(--color-accent)]"
      : "hover:bg-black/[0.03] dark:hover:bg-white/[0.04]",
  );

function LiveDot({ state }: { state: "busy" | "waiting" | "idle" }) {
  return (
    <span
      data-testid="live-dot"
      data-state={state}
      title={state === "busy" ? "工作中" : state === "waiting" ? "等待作答" : "空闲"}
      className={cn(
        "size-2 shrink-0 rounded-full",
        state === "busy" && "bg-live shadow-[0_0_0_3px_rgba(30,127,69,0.16)]",
        state === "waiting" &&
          "bg-live-waiting shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-live-waiting)_20%,transparent)]",
        state === "idle" && "box-border border-[1.5px] border-live",
      )}
    />
  );
}

/** A Session in `ccrpc ls` before its first prompt: no transcript yet, so only its directory. */
function PendingRow({ p, selected, onOpen }: { p: PendingSession; selected: boolean; onOpen: (id: string) => void }) {
  const unread = useUnread(p.id);
  return (
    <div
      role="option"
      aria-selected={selected}
      data-testid="session-row"
      data-session-id={p.id}
      data-pending=""
      onClick={() => onOpen(p.id)}
      className={rowClass(selected)}
    >
      <div className="flex items-center gap-1.5">
        <LiveDot state={p.status} />
        <span className="min-w-0 flex-1 truncate font-semibold">新 Session</span>
        {unread && <span data-testid="unread" title="有新回复" className="size-1.5 shrink-0 rounded-full bg-accent" />}
      </div>
      {p.cwd && (
        <div className="flex items-center gap-[3px] text-[11px] text-secondary" title={p.cwd}>
          <Folder size={11} strokeWidth={1.5} className="shrink-0" aria-hidden />
          <span className="truncate">{p.cwd.split("/").filter(Boolean).pop() ?? p.cwd}</span>
        </div>
      )}
    </div>
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
