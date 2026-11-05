import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { Command } from "cmdk";
import { Folder, GitBranch, Search, X } from "lucide-react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { LiveDot } from "@/components/LiveDot";
import { api } from "@/ipc";
import type {
  AppError,
  ProjectSummary,
  SearchGroup,
  SearchHit,
  SearchMode,
  SearchRequest,
  SearchResponse,
  SessionQuery,
  SessionSummary,
} from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatRelative } from "@/lib/format";
import { isComposingKey, keepOpenWhileComposing, useHotkey } from "@/lib/hotkeys";
import { queryKeys, useIndexStatus, useProjects, useSessions } from "@/queries";
import { useUi } from "@/state/ui";
import { highlight, matchProjects, matchSessions, parseQuery, sessionProjectIds } from "./match";
import { errorText } from "@/lib/errors";

const nf = new Intl.NumberFormat("en-US");
const MAX_PROJECTS = 3;
const MAX_SESSIONS = 8;
const MAX_RECENT = 20;
const HITS_PER_SESSION = 5;
const ALL_RECENT: SessionQuery = { projectIds: [], sort: "lastActive", descending: true, timeRange: null, drill: null };
const MODE_HINT: Partial<Record<SearchMode, string>> = { like: "短词匹配", mixed: "短词匹配" };
const NO_GROUPS: SearchGroup[] = [];

const SECTION = "flex items-center gap-2 px-4 pt-2.5 pb-1 text-[11px] font-semibold text-secondary";
const ITEM = "group flex cursor-default items-center gap-2.5 px-4 py-[7px] data-[selected=true]:bg-selection";

/** `i` (the hit's place in its group) keeps two hits in different blocks of one entry apart. */
const hitKey = (sessionId: string, h: SearchHit, i: number) =>
  `h:${sessionId}|${h.nodeId}|${h.agentId ?? ""}|${h.toolUseId ?? ""}|${h.role}|${i}`;

/** Keeps focus in the input when a button inside the dialog is clicked. */
const keepFocus = (e: MouseEvent) => e.preventDefault();

function Marked({ text, terms }: { text: string; terms: string[] }) {
  return (
    <>
      {highlight(text, terms).map((p, i) =>
        p.hit ? (
          <mark key={i} className="rounded-[2px] bg-mark px-px text-mark-text">
            {p.text}
          </mark>
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </>
  );
}

/** Shown on the selected row only. */
function Hint({ children }: { children: ReactNode }) {
  return <span className="hidden shrink-0 text-[11px] text-accent group-data-[selected=true]:inline">{children}</span>;
}

function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded border border-b-2 border-border bg-ground px-1 font-sans text-[11px] text-text">
      {children}
    </kbd>
  );
}

function ProjectItem({
  p,
  terms,
  onFilter,
  onScope,
}: {
  p: ProjectSummary;
  terms: string[];
  onFilter: () => void;
  onScope: () => void;
}) {
  return (
    <Command.Item value={`p:${p.id}`} onSelect={onFilter} data-testid="search-project" className={cn(ITEM, "py-2")}>
      <Folder
        size={15}
        strokeWidth={1.5}
        className="shrink-0 text-secondary group-data-[selected=true]:text-accent"
        aria-hidden
      />
      <span className="shrink-0 font-semibold">
        <Marked text={p.displayName} terms={terms} />
      </span>
      <span className="min-w-0 flex-1 truncate text-[12px] text-secondary">
        <Marked text={p.path} terms={terms} /> · {p.missing ? "已不存在 · " : ""}
        {p.sessionCount} 个 Session
      </span>
      <span className="hidden shrink-0 gap-1.5 group-data-[selected=true]:flex">
        <button type="button" className="flex h-6 items-center gap-1 rounded-md bg-accent px-2 text-[12px] text-white">
          <Kbd>↵</Kbd> 筛选 Session 列表
        </button>
        <button
          type="button"
          onMouseDown={keepFocus}
          onClick={(e) => {
            e.stopPropagation();
            onScope();
          }}
          className="flex h-6 items-center gap-1 rounded-md border border-accent-border bg-ground px-2 text-[12px] text-accent"
        >
          <Kbd>⌘↵</Kbd> 只搜这个 Project
        </button>
      </span>
    </Command.Item>
  );
}

function SessionItem({
  s,
  names,
  terms,
  onSelect,
}: {
  s: SessionSummary;
  names: Map<string, string>;
  terms: string[];
  onSelect: () => void;
}) {
  const tags = sessionProjectIds(s).map((id) => names.get(id) ?? id);
  const liveStatus = s.live?.status;
  return (
    <Command.Item value={`s:${s.id}`} onSelect={onSelect} data-testid="search-session" className={ITEM}>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex items-center gap-1.5">
          {s.live && (
            <span
              className={cn(
                "size-2 shrink-0 rounded-full",
                liveStatus === "busy" && "bg-live",
                liveStatus === "waiting" &&
                  "bg-live-waiting shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-live-waiting)_20%,transparent)]",
                liveStatus !== "busy" && liveStatus !== "waiting" && "box-border border-[1.5px] border-live",
              )}
            />
          )}
          <span className="min-w-0 truncate font-semibold">
            <Marked text={s.title} terms={terms} />
          </span>
        </div>
        <div className="flex items-center gap-2.5 text-[11px] text-secondary">
          <span className="min-w-0 truncate text-text/80">
            <Marked text={tags.join(" · ")} terms={terms} />
          </span>
          {s.gitBranch && (
            <span className="flex min-w-0 shrink-0 items-center gap-[3px]">
              <GitBranch size={11} strokeWidth={1.5} className="shrink-0" aria-hidden />
              <span className="max-w-[200px] truncate">
                <Marked text={s.gitBranch} terms={terms} />
              </span>
            </span>
          )}
        </div>
      </div>
      <Hint>↵ 打开</Hint>
      <span className="shrink-0 text-[11px] text-secondary">{formatRelative(s.lastActiveMs)}</span>
    </Command.Item>
  );
}

/** `disabled`: a result of an earlier query, shown until the current one returns but not selectable. */
function HitRow({
  value,
  hit,
  disabled,
  onOpen,
}: {
  value: string;
  hit: SearchHit;
  disabled: boolean;
  onOpen: () => void;
}) {
  const agentType = hit.agentId ? hit.agentType : null;
  const role = hit.agentId
    ? (agentType ?? "Subagent")
    : hit.role === "user"
      ? "你"
      : hit.role === "assistant"
        ? "Claude"
        : "工具输入";
  const mono = hit.role === "toolInput";
  return (
    <Command.Item
      value={value}
      onSelect={onOpen}
      disabled={disabled}
      data-testid="search-hit"
      className="group flex cursor-default items-baseline gap-2.5 py-[3px] pr-4 pl-8 data-[disabled=true]:opacity-50 data-[selected=true]:bg-selection"
    >
      <span className="w-[60px] shrink-0 truncate text-[12px] text-secondary">{role}</span>
      <span className={cn("min-w-0 flex-1 truncate text-[12px] text-text/80", mono && "font-mono")}>
        {hit.snippet.map((p, i) =>
          p.hit ? (
            <mark key={i} className="rounded-[2px] bg-mark px-px text-mark-text">
              {p.text}
            </mark>
          ) : (
            <span key={i}>{p.text}</span>
          ),
        )}
      </span>
      {hit.agentId && (
        <span
          data-testid="tag-subagent"
          className="shrink-0 rounded border border-tag-border bg-tag px-1.5 text-[11px] text-tag-text"
        >
          Subagent{agentType ? ` · ${agentType}` : ""}
        </span>
      )}
      {!hit.onMainLine && (
        <span
          data-testid="tag-branch"
          className="shrink-0 rounded border border-tag-border bg-tag px-1.5 text-[11px] text-tag-text"
        >
          已回退的分支
        </span>
      )}
      <Hint>↵ 打开并定位</Hint>
    </Command.Item>
  );
}

/**
 * ⌘K: one dialog for Projects (pick to filter the Session list, ⌘↵ to search only it), Sessions
 * (title, Project, branch; matched here, instantly) and conversation content (full-text index, debounced).
 * Later sections only append below, so the selection never jumps.
 */
export function SearchOverlay() {
  const open = useUi((s) => s.searchOpen);
  const setOpen = useUi((s) => s.setSearchOpen);
  const toggle = () => {
    const ui = useUi.getState();
    ui.setStatsOpen(false);
    setOpen(!ui.searchOpen);
  };
  useHotkey("Meta+K", toggle, { enableInInputs: true });

  const [input, setInput] = useState("");
  // IME composition (e.g. pinyin) puts unconfirmed text in the input; match only confirmed text.
  const [composing, setComposing] = useState(false);
  const [text, setText] = useState("");
  const [ftsText, setFtsText] = useState("");
  /** Project the search is limited to (⌘↵ on a Project). */
  const [scope, setScope] = useState<string | null>(null);
  const [selected, setSelected] = useState("");
  const [jumpError, setJumpError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // The input unmounts on close, possibly mid-composition (no compositionend follows).
  useEffect(() => {
    if (open) setJumpError(null);
    else setComposing(false);
  }, [open]);
  useEffect(() => {
    if (!composing) setText(input);
  }, [input, composing]);
  useEffect(() => {
    const t = setTimeout(() => setFtsText(text), 200);
    return () => clearTimeout(t);
  }, [text]);

  // Loaded only while open: closed, it would refetch every session on each change of a Live Session.
  const sessions = useSessions(ALL_RECENT, open).data;
  const projects = useProjects().data;
  const status = useIndexStatus().data;
  const names = useMemo(() => new Map((projects ?? []).map((p) => [p.id, p.displayName])), [projects]);
  const scopeName = scope ? (names.get(scope) ?? scope) : null;

  const blank = text.trim() === "";
  const parsed = useMemo(() => parseQuery(text), [text]);
  const inScope = useMemo(
    () => (scope ? (sessions ?? []).filter((s) => sessionProjectIds(s).includes(scope)) : (sessions ?? [])),
    [sessions, scope],
  );
  const sessionHits = useMemo(
    () => (blank ? inScope.slice(0, MAX_RECENT) : matchSessions(inScope, names, parsed).slice(0, MAX_SESSIONS)),
    [blank, inScope, names, parsed],
  );
  const projectHits = useMemo(
    () => (blank || scope ? [] : matchProjects(projects ?? [], parsed).slice(0, MAX_PROJECTS)),
    [blank, scope, projects, parsed],
  );

  const ftsQuery = ftsText.trim();
  const req = useMemo<SearchRequest>(
    () => ({ query: ftsQuery, projectId: scope, maxSessions: 50, hitsPerSession: HITS_PER_SESSION }),
    [ftsQuery, scope],
  );
  // An all-exclusion query is rejected by the backend; there is nothing to search for.
  const active = open && !blank && parseQuery(ftsQuery).include.length > 0;
  const search = useQuery<SearchResponse, AppError>({
    queryKey: queryKeys.search(req),
    queryFn: () => api.search(req),
    enabled: active,
    staleTime: 0,
    placeholderData: keepPreviousData,
  });
  // keepPreviousData also fills disabled queries; a blank query shows no content results.
  const data = active ? search.data : undefined;
  const groups = data?.groups ?? NO_GROUPS;
  // Results of an earlier query or scope, kept on screen until the current one returns.
  const stale = ftsQuery !== text.trim() || search.isPlaceholderData;
  const searching = !blank && parsed.include.length > 0 && (stale || search.isFetching);
  const contentSettled = active && !searching && data != null && !search.error;

  // Every selectable value in display order, by section.
  const sections = useMemo(
    () =>
      [
        projectHits.map((p) => `p:${p.id}`),
        sessionHits.map((s) => `s:${s.id}`),
        stale ? [] : groups.flatMap((g) => g.hits.slice(0, HITS_PER_SESSION).map((h, i) => hitKey(g.session.id, h, i))),
      ].filter((keys) => keys.length > 0),
    [projectHits, sessionHits, groups, stale],
  );
  const allKeys = useMemo(() => sections.flat(), [sections]);
  // A new query or scope selects the first result; arriving results keep the selection unless it vanished.
  useEffect(() => {
    setSelected(allKeys[0] ?? "");
    // Keyed on the query only: results arriving for it go through the next effect instead.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, scope, open]);
  useEffect(() => {
    if (!allKeys.includes(selected)) setSelected(allKeys[0] ?? "");
  }, [allKeys, selected]);

  const close = () => setOpen(false);
  /** Opens a session; a Session list filter that would hide it is cleared. */
  const showSession = (s: SessionSummary) => {
    const ui = useUi.getState();
    if (ui.projectIds.length && !sessionProjectIds(s).some((id) => ui.projectIds.includes(id))) ui.setProjectIds([]);
  };
  const openSession = (s: SessionSummary) => {
    showSession(s);
    const ui = useUi.getState();
    ui.selectSession(s.id);
    close();
  };
  const openHit = async (g: SearchGroup, hit: SearchHit) => {
    try {
      const target = await api.resolveJump({
        sessionId: g.session.id,
        nodeId: hit.nodeId,
        agentId: hit.agentId,
        toolUseId: hit.toolUseId,
      });
      // Closed meanwhile: the jump is no longer wanted.
      if (!useUi.getState().searchOpen) return;
      showSession(g.session);
      useUi.getState().jumpTo(target);
      close();
    } catch (e) {
      setJumpError(errorText(e));
    }
  };
  const filterProject = (id: string) => {
    const ui = useUi.getState();
    ui.setProjectIds([id]);
    // Picking a Project is a request to see its Sessions; a collapsed list would show nothing.
    if (ui.collapsed.sessions) ui.toggleCollapsed("sessions");
    close();
  };
  const scopeTo = (id: string) => {
    setScope(id);
    setInput("");
    inputRef.current?.focus();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (isComposingKey(e.nativeEvent)) return;
    if (e.key === "Tab" && sections.length > 0) {
      e.preventDefault();
      const at = sections.findIndex((keys) => keys.includes(selected));
      const next = (at + (e.shiftKey ? -1 : 1) + sections.length) % sections.length;
      setSelected(sections[next]![0]!);
    } else if (e.key === "Enter" && e.metaKey) {
      e.preventDefault();
      if (selected.startsWith("p:")) scopeTo(selected.slice(2));
    } else if (e.key === "Backspace" && !e.repeat && input === "" && scope) {
      e.preventDefault();
      setScope(null);
    }
  };

  const textProgress =
    status && !status.textReady && status.filesTotal > 0
      ? `${nf.format(status.filesDone)} / ${nf.format(status.filesTotal)} 个文件`
      : null;
  const indexIncomplete = (status != null && !status.textReady) || data?.indexComplete === false;
  const totalHits = data?.totalHits ?? 0;
  const errMsg = (active ? search.error?.message : null) ?? jumpError;
  const kind = selected.slice(0, 2);

  return (
    <DialogPrimitive.Root open={open} onOpenChange={setOpen}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-[rgba(28,28,30,0.28)]" />
        <DialogPrimitive.Content
          data-testid="search"
          aria-describedby={undefined}
          onEscapeKeyDown={keepOpenWhileComposing}
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            // The last query is kept; selecting it lets typing replace it.
            (e.currentTarget as HTMLElement).querySelector<HTMLInputElement>("[cmdk-input]")?.select();
          }}
          className="fixed top-[72px] left-1/2 z-50 flex max-h-[min(780px,calc(100vh-96px))] w-[min(780px,calc(100vw-32px))] -translate-x-1/2 flex-col overflow-hidden rounded-[14px] border border-border bg-ground text-text shadow-[0_24px_64px_rgba(0,0,0,0.22),0_2px_8px_rgba(0,0,0,0.08)]"
        >
          <DialogPrimitive.Title className="sr-only">搜索</DialogPrimitive.Title>
          <Command
            shouldFilter={false}
            loop
            label="搜索 Session、Project 或对话内容"
            value={selected}
            onValueChange={setSelected}
            onKeyDown={onKeyDown}
            className="flex min-h-0 flex-1 flex-col"
          >
            <div className="flex items-center gap-2.5 border-b border-border px-4 py-3.5">
              <Search size={18} strokeWidth={1.6} className="shrink-0 text-secondary" aria-hidden />
              {scopeName && (
                <span
                  data-testid="search-scope"
                  className="flex h-[26px] shrink-0 items-center gap-1 rounded-md bg-selection pr-1 pl-2 text-[13px] font-semibold text-accent"
                >
                  <Folder size={13} strokeWidth={1.8} aria-hidden />
                  {scopeName}
                  <button
                    type="button"
                    aria-label="移除 Project 范围"
                    onMouseDown={keepFocus}
                    onClick={() => {
                      setScope(null);
                      inputRef.current?.focus();
                    }}
                    className="rounded-full p-0.5 hover:bg-black/10"
                  >
                    <X size={12} strokeWidth={2} aria-hidden />
                  </button>
                </span>
              )}
              <Command.Input
                ref={inputRef}
                value={input}
                onValueChange={(v) => {
                  setInput(v);
                  setJumpError(null);
                }}
                onCompositionStart={() => setComposing(true)}
                onCompositionEnd={() => setComposing(false)}
                placeholder={scopeName ? `在 ${scopeName} 中搜索` : "搜索 Session、Project 或对话内容"}
                className="min-w-0 flex-1 bg-transparent text-[17px] outline-none placeholder:text-secondary"
              />
              <span className="shrink-0 text-[12px] text-secondary">esc 关闭</span>
            </div>
            {indexIncomplete && !blank && (
              <div
                data-testid="search-incomplete"
                className="flex items-center gap-3 border-b border-border bg-list px-4 py-1.5 text-[11px] text-secondary"
              >
                <span>{textProgress ? `全文索引未完成 · ${textProgress}` : "全文索引未完成"}</span>
                {status && status.filesTotal > 0 && (
                  <span className="h-1 w-28 overflow-hidden rounded-full bg-border">
                    <span
                      className="block h-full rounded-full bg-accent"
                      style={{ width: `${Math.min(100, (status.filesDone / status.filesTotal) * 100)}%` }}
                    />
                  </span>
                )}
                <span className="ml-auto">Session 与 Project 的匹配不受影响</span>
              </div>
            )}
            {errMsg && (
              <div
                role="alert"
                data-testid="search-error"
                className="border-b border-border px-4 py-1.5 text-[12px] text-error"
              >
                {errMsg}
              </div>
            )}

            <Command.List className="min-h-0 flex-1 overflow-y-auto pb-2">
              {projectHits.length > 0 && (
                <Command.Group heading={<div className={SECTION}>Project</div>}>
                  {projectHits.map((p) => (
                    <ProjectItem
                      key={p.id}
                      p={p}
                      terms={parsed.include}
                      onFilter={() => filterProject(p.id)}
                      onScope={() => scopeTo(p.id)}
                    />
                  ))}
                </Command.Group>
              )}

              {sessions && (
                <Command.Group
                  heading={
                    <div className={SECTION}>
                      {blank ? (scopeName ? `${scopeName} 最近的 Session` : "最近的 Session") : "Session"}
                      {!blank && (
                        <span className="font-normal">
                          · {scopeName ? `在 ${scopeName} 中` : "标题、Project、分支"}
                        </span>
                      )}
                    </div>
                  }
                >
                  {sessionHits.map((s) => (
                    <SessionItem
                      key={s.id}
                      s={s}
                      names={names}
                      terms={parsed.include}
                      onSelect={() => openSession(s)}
                    />
                  ))}
                  {sessionHits.length === 0 && (
                    <div data-testid="search-session-empty" className="px-4 pt-1 pb-1.5 text-[12px] text-secondary">
                      {blank ? "没有 Session。" : "没有标题、Project 或分支匹配的 Session。"}
                    </div>
                  )}
                </Command.Group>
              )}

              {!blank && (
                <Command.Group
                  heading={
                    <div className={cn(SECTION, "pt-3.5")}>
                      对话内容
                      {contentSettled && groups.length > 0 && (
                        <span data-testid="search-summary" className="font-normal">
                          · {nf.format(data.totalSessions)} 个 Session，共 {nf.format(totalHits)} 处
                        </span>
                      )}
                      {data?.mode && MODE_HINT[data.mode] && (
                        <span className="font-normal">· {MODE_HINT[data.mode]}</span>
                      )}
                      {searching && (
                        <>
                          <span
                            className="size-[11px] animate-spin rounded-full border-[1.5px] border-border border-t-accent"
                            aria-hidden
                          />
                          <span className="font-normal">搜索中…</span>
                        </>
                      )}
                    </div>
                  }
                >
                  {parsed.include.length === 0 && (
                    <div className="px-4 pt-1 pb-2 text-[12px] text-secondary">至少需要一个包含词。</div>
                  )}
                  {contentSettled && groups.length === 0 && (
                    <div role="status" data-testid="search-empty" className="flex flex-col gap-1.5 px-4 pt-1 pb-2">
                      <div className="font-semibold">没有找到 “{ftsQuery}”</div>
                      {indexIncomplete && (
                        <div className="text-[12px] text-text/80">全文索引未完成，建好后可能有结果。</div>
                      )}
                    </div>
                  )}
                  {groups.map((g) => (
                    <div
                      key={g.session.id}
                      data-testid="search-group"
                      className="flex flex-col gap-0.5 border-t border-border/70 py-2"
                    >
                      <div className="flex items-center gap-2 px-4 pb-0.5">
                        {g.session.live && <LiveDot state="busy" testId="search-live" />}
                        <span className="min-w-0 truncate font-semibold">{g.session.title}</span>
                        <span className="shrink-0 text-[12px] text-secondary">
                          {sessionProjectIds(g.session)
                            .map((id) => names.get(id) ?? id)
                            .join(" · ")}{" "}
                          · {formatRelative(g.session.lastActiveMs)}
                        </span>
                        <span className="flex-1" />
                        <span className="shrink-0 rounded-full bg-code px-2 text-[11px]">{g.hitCount} 处</span>
                      </div>
                      {g.hits.slice(0, HITS_PER_SESSION).map((h, i) => {
                        const key = hitKey(g.session.id, h, i);
                        return (
                          <HitRow key={key} value={key} hit={h} disabled={stale} onOpen={() => void openHit(g, h)} />
                        );
                      })}
                    </div>
                  ))}
                </Command.Group>
              )}
            </Command.List>

            <div className="flex flex-wrap items-center gap-4 border-t border-border bg-list px-4 py-2 text-[11px] text-secondary">
              <span>
                <Kbd>↑</Kbd>
                <Kbd>↓</Kbd> 选择
              </span>
              <span>
                <Kbd>⇥</Kbd> 下一段
              </span>
              {kind === "p:" ? (
                <>
                  <span>
                    <Kbd>↵</Kbd> 筛选列表
                  </span>
                  <span>
                    <Kbd>⌘↵</Kbd> 限定搜索范围
                  </span>
                </>
              ) : (
                <span>
                  <Kbd>↵</Kbd> {kind === "h:" ? "打开并定位" : "打开"}
                </span>
              )}
              {scope ? (
                <span>
                  输入为空时 <Kbd>⌫</Kbd> 移除 Project 范围
                </span>
              ) : (
                <>
                  <span>"短语" 精确匹配</span>
                  <span>-词 排除</span>
                </>
              )}
              <span className="ml-auto">⌘K 打开或关闭</span>
            </div>
          </Command>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
