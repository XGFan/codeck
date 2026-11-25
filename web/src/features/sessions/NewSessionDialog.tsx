import { Command } from "cmdk";
import { Folder, FolderSearch, SquarePen } from "lucide-react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { useMemo, useState } from "react";
import { Dialog, PanelDialogContent } from "@/components/ui/dialog";
import { useConnect } from "@/features/connect";
import { formatRelative } from "@/lib/format";
import { keepOpenWhileComposing, useHotkey } from "@/lib/hotkeys";
import { useUi } from "@/state/ui";
import { isUnder, type Sidebar } from "./groups";
import type { ProjectSummary } from "@/ipc/bindings";

/** `/Users/<name>/…` as `~/…`. */
export const tildePath = (path: string) => path.replace(/^\/Users\/[^/]+(?=\/|$)/, "~");

interface Target {
  path: string;
  name: string;
  timeMs: number;
  /** Under a nesting group: its group's name. */
  within: string | null;
}

/**
 * 「新建 Session」 (⌘N): the added Projects, latest first; typing also matches the Projects gathered into a nesting
 * group. ↵ starts the Session in that directory, 「选择其他目录…」 (⌘O) picks another one.
 */
export function NewSessionDialog({ sidebar, projects }: { sidebar: Sidebar | null; projects: ProjectSummary[] }) {
  const open = useUi((s) => s.newSessionOpen);
  const setOpen = useUi((s) => s.setNewSessionOpen);
  const newSession = useConnect((s) => s.newSession);
  const [query, setQuery] = useState("");

  const targets = useMemo(() => {
    const out: Target[] = [];
    const groups = sidebar?.groups ?? [];
    const added = new Set(groups.map((g) => g.path));
    for (const g of groups) {
      out.push({ path: g.path, name: g.name, timeMs: g.lastMs, within: null });
      if (!g.nest) continue;
      for (const p of projects) {
        // Added on its own: listed as its own group already.
        if (added.has(p.path)) continue;
        if (isUnder(p.path, g.path))
          out.push({ path: p.path, name: p.displayName, timeMs: p.lastActiveMs, within: g.name });
      }
    }
    return out;
  }, [sidebar, projects]);
  const q = query.trim().toLowerCase();
  const shown = targets.filter((t) =>
    q ? t.name.toLowerCase().includes(q) || t.path.toLowerCase().includes(q) : t.within == null,
  );

  const close = () => {
    setOpen(false);
    setQuery("");
  };
  useHotkey("Meta+N", () => (useUi.getState().newSessionOpen ? close() : setOpen(true)), { enableInInputs: true });
  const start = (path?: string) => {
    close();
    void newSession(path);
  };

  return (
    <Dialog open={open} onOpenChange={(o) => (o ? setOpen(true) : close())}>
      <PanelDialogContent
        data-testid="new-session-dialog"
        aria-describedby={undefined}
        onEscapeKeyDown={keepOpenWhileComposing}
        className="top-[72px] flex max-h-[min(640px,calc(100vh-96px))] w-[min(660px,calc(100vw-32px))] flex-col bg-ground"
      >
        <DialogPrimitive.Title className="sr-only">新建 Session</DialogPrimitive.Title>
        <Command
          shouldFilter={false}
          loop
          label="在哪个 Project 新建 Session"
          onKeyDown={(e) => {
            if (e.key === "o" && e.metaKey) {
              e.preventDefault();
              start();
            }
          }}
          className="flex min-h-0 flex-1 flex-col"
        >
          <div className="flex items-center gap-2.5 border-b border-border px-4 py-3.5">
            <SquarePen size={18} strokeWidth={1.6} className="shrink-0 text-secondary" aria-hidden />
            <Command.Input
              value={query}
              onValueChange={setQuery}
              placeholder="在哪个 Project 新建 Session"
              className="min-w-0 flex-1 bg-transparent text-[16px] outline-none placeholder:text-secondary"
            />
            <span className="shrink-0 text-[12px] text-secondary">esc 关闭</span>
          </div>
          <Command.List className="min-h-0 flex-1 overflow-y-auto pb-1.5">
            <Command.Group
              heading={
                <div className="px-4 pt-2.5 pb-1 text-[11px] font-semibold text-secondary">已添加的 Project</div>
              }
            >
              {shown.map((t) => (
                <Command.Item
                  key={t.path}
                  value={t.path}
                  onSelect={() => start(t.path)}
                  data-testid="new-session-target"
                  className={ITEM}
                >
                  <Folder size={15} strokeWidth={1.5} className="shrink-0 text-secondary" aria-hidden />
                  <span className="flex min-w-0 flex-1 flex-col gap-px">
                    <span className="font-semibold">
                      {t.name}
                      {t.within && <span className="ml-1.5 text-[11px] font-normal text-secondary">{t.within}</span>}
                    </span>
                    <span className="truncate text-[11px] text-secondary">{tildePath(t.path)}</span>
                  </span>
                  <span className="hidden shrink-0 text-[11px] text-accent group-data-[selected=true]:inline">
                    ↵ 新建
                  </span>
                  {t.timeMs > 0 && (
                    <span className="shrink-0 text-[11px] text-secondary">{formatRelative(t.timeMs)}</span>
                  )}
                </Command.Item>
              ))}
              {shown.length === 0 && (
                <div className="px-4 py-1.5 text-[12px] text-secondary">
                  {q ? "没有匹配的 Project。" : "还没有添加 Project。"}
                </div>
              )}
            </Command.Group>
            <div className="my-1 h-px bg-border" />
            <Command.Item value="other" onSelect={() => start()} data-testid="new-session-other" className={ITEM}>
              <FolderSearch size={15} strokeWidth={1.5} className="shrink-0 text-secondary" aria-hidden />
              <span className="flex-1">选择其他目录…</span>
              <span className="shrink-0 text-[11px] text-secondary">⌘O</span>
            </Command.Item>
          </Command.List>
          <div className="flex flex-wrap gap-4 border-t border-border bg-list px-4 py-2 text-[11px] text-secondary">
            <span>
              <Kbd>↑</Kbd>
              <Kbd>↓</Kbd> 选择
            </span>
            <span>
              <Kbd>↵</Kbd> 在该目录新建
            </span>
            <span>
              <Kbd>⌘O</Kbd> 其他目录
            </span>
            <span className="ml-auto">⌘N 打开或关闭</span>
          </div>
        </Command>
      </PanelDialogContent>
    </Dialog>
  );
}

const ITEM = "group flex cursor-default items-center gap-2.5 px-4 py-[7px] data-[selected=true]:bg-selection";

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="mx-px rounded border border-b-2 border-border bg-ground px-1 font-[inherit] text-[11px] text-text">
      {children}
    </kbd>
  );
}
