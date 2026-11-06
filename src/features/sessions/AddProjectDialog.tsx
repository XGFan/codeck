import { FolderSearch, Search } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { api } from "@/ipc";
import type { ProjectSummary, SidebarProject } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCount, formatRelative } from "@/lib/format";
import { keepOpenWhileComposing } from "@/lib/hotkeys";
import { useUi } from "@/state/ui";
import { childProjects, suggested } from "./groups";
import { tildePath } from "./NewSessionDialog";

/** Rows listed while nothing is typed; typing searches every Project. */
const LISTED = 50;

/**
 * 「添加 Project」: tick indexed Projects not in the sidebar yet (latest first), or pick another directory. A Project
 * with others under it offers 「收拢子 Project」. The first one (`entries` null) ticks the recently active Projects
 * and, closed without adding, still counts as set up (an empty sidebar).
 */
export function AddProjectDialog({
  projects,
  entries,
  onSave,
}: {
  projects: ProjectSummary[];
  entries: SidebarProject[] | null;
  onSave: (entries: SidebarProject[]) => void;
}) {
  const open = useUi((s) => s.addProjectOpen);
  const setOpen = useUi((s) => s.setAddProjectOpen);
  const [query, setQuery] = useState("");
  /** Ticked paths → 「收拢子 Project」. */
  const [ticked, setTicked] = useState<Map<string, boolean>>(new Map());
  /** Directories picked with 「选择其他目录…」 (ticked as they come). */
  const [picked, setPicked] = useState<string[]>([]);
  const firstRun = entries === null;

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setPicked([]);
    setTicked(firstRun ? new Map([...suggested(projects, Date.now())].map((p) => [p, false])) : new Map());
    // Only when it opens: the lists below follow `projects` as the index changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const added = useMemo(() => new Set((entries ?? []).map((e) => e.path)), [entries]);
  const candidates = useMemo(
    () => [...projects].filter((p) => !added.has(p.path)).sort((a, b) => b.lastActiveMs - a.lastActiveMs),
    [projects, added],
  );
  const q = query.trim().toLowerCase();
  const shown = (
    q
      ? candidates.filter((p) => p.displayName.toLowerCase().includes(q) || p.path.toLowerCase().includes(q))
      : candidates.slice(0, LISTED)
  ).filter((p) => !picked.includes(p.path));

  const toggle = (path: string) =>
    setTicked((m) => {
      const next = new Map(m);
      if (next.has(path)) next.delete(path);
      else next.set(path, false);
      return next;
    });
  const setNest = (path: string, nest: boolean) => setTicked((m) => new Map(m).set(path, nest));

  const close = () => {
    if (firstRun) onSave([]);
    setOpen(false);
  };
  const save = () => {
    onSave([...(entries ?? []), ...[...ticked].map(([path, nest]) => ({ path, nest }))]);
    setOpen(false);
  };
  const pickOther = async () => {
    const dir = await api.pickDir();
    if (!dir || added.has(dir)) return;
    // Listed at the top even when it is a candidate the filter or the first 50 leave out.
    setPicked((p) => (p.includes(dir) ? p : [...p, dir]));
    setTicked((m) => new Map(m).set(dir, m.get(dir) ?? false));
  };

  const row = (path: string, name: string, meta: string | null, children: number) => {
    const on = ticked.has(path);
    return (
      <div key={path} className={cn("flex flex-col", on && children > 0 && "bg-selection/40")}>
        <label
          data-testid="add-project-row"
          className="flex min-h-10 cursor-default items-center gap-2.5 px-[18px] py-1.5"
        >
          <input type="checkbox" checked={on} onChange={() => toggle(path)} className="size-4 shrink-0 accent-accent" />
          <span className="flex min-w-0 flex-1 flex-col gap-px">
            <span className="flex items-center gap-1.5 font-semibold">
              <span className="truncate">{name}</span>
              {children > 0 && (
                <span className="shrink-0 rounded bg-text/8 px-1.5 text-[11px] font-medium text-secondary">
                  含 {formatCount(children)} 个子 Project
                </span>
              )}
            </span>
            <span className="truncate text-[11px] text-secondary">{tildePath(path)}</span>
          </span>
          {meta && <span className="shrink-0 text-[11px] text-secondary">{meta}</span>}
        </label>
        {on && children > 0 && (
          <label className="mb-1.5 ml-11 mr-[18px] flex cursor-default items-center gap-2.5 rounded-lg border border-accent-border bg-ground px-2.5 py-2">
            <input
              type="checkbox"
              role="switch"
              aria-label={`收拢 ${name} 的子 Project`}
              checked={ticked.get(path) ?? false}
              onChange={(e) => setNest(path, e.target.checked)}
              className="size-4 shrink-0 accent-accent"
            />
            <span className="flex flex-col gap-0.5 text-[12px]">
              <span>收拢子 Project</span>
              <span className="text-[11px] text-secondary">
                它下面 {formatCount(children)} 个 Project 的 Session 并入这一组
              </span>
            </span>
          </label>
        )}
      </div>
    );
  };

  return (
    <Dialog open={open} onOpenChange={(o) => (o ? setOpen(true) : close())}>
      <DialogContent
        data-testid="add-project"
        onEscapeKeyDown={keepOpenWhileComposing}
        className="flex max-h-[min(720px,calc(100vh-64px))] w-[min(660px,calc(100vw-32px))] flex-col p-0"
      >
        <div className="flex flex-col gap-1 px-[18px] pt-4 pb-3">
          <DialogTitle className="mb-0">添加 Project</DialogTitle>
          <DialogDescription className="text-[12px] text-secondary">
            侧栏只显示添加的 Project，其余 Session 在「其他」里。
          </DialogDescription>
        </div>
        <label className="mx-[18px] mb-1.5 flex h-8 items-center gap-2 rounded-lg border border-border px-2.5">
          <Search size={14} strokeWidth={1.8} className="shrink-0 text-secondary" aria-hidden />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="按名称或路径筛选"
            aria-label="按名称或路径筛选"
            className="min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-secondary"
          />
        </label>
        <div className="min-h-0 flex-1 overflow-y-auto pb-1.5">
          {picked.map((path) => {
            const p = candidates.find((c) => c.path === path);
            return row(
              path,
              p?.displayName ?? path.split("/").filter(Boolean).pop() ?? path,
              p ? `${formatCount(p.sessionCount)} 个 · ${formatRelative(p.lastActiveMs)}` : null,
              childProjects(path, projects),
            );
          })}
          {shown.map((p) =>
            row(
              p.path,
              p.displayName,
              `${formatCount(p.sessionCount)} 个 · ${formatRelative(p.lastActiveMs)}`,
              childProjects(p.path, projects),
            ),
          )}
          {shown.length === 0 && picked.length === 0 && (
            <div className="px-[18px] py-2 text-[12px] text-secondary">
              {q ? "没有匹配的 Project。" : "索引里的 Project 都已添加。"}
            </div>
          )}
        </div>
        <div className="flex items-center gap-2 border-t border-border bg-list px-[18px] py-2.5">
          <button type="button" onClick={() => void pickOther()} className={BUTTON}>
            <FolderSearch size={13} strokeWidth={1.7} aria-hidden />
            选择其他目录…
          </button>
          <span className="flex-1" />
          <span className="text-[12px] text-secondary">已选 {ticked.size} 个</span>
          <button type="button" onClick={close} className={BUTTON}>
            取消
          </button>
          <button
            type="button"
            disabled={ticked.size === 0}
            onClick={save}
            className={cn(BUTTON, "border-accent bg-accent font-semibold text-white disabled:opacity-40")}
          >
            添加
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

const BUTTON =
  "flex h-7 items-center gap-1.5 rounded-md border border-border bg-ground px-3 text-[12px] text-text outline-none focus-visible:outline-2 focus-visible:outline-accent";
