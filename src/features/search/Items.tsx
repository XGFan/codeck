import { Command } from "cmdk";
import { Folder, GitBranch } from "lucide-react";
import type { MouseEvent, ReactNode } from "react";
import type { ProjectSummary, SearchHit, SessionSummary } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatRelative } from "@/lib/format";
import { highlight, sessionProjectIds } from "./match";
import { hitRole } from "./select";

const ITEM = "group flex cursor-default items-center gap-2.5 px-4 py-[7px] data-[selected=true]:bg-selection";

/** Keeps focus in the input when a button inside the dialog is clicked. */
export const keepFocus = (e: MouseEvent) => e.preventDefault();

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

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded border border-b-2 border-border bg-ground px-1 font-sans text-[11px] text-text">
      {children}
    </kbd>
  );
}

export function ProjectItem({
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

export function SessionItem({
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
export function HitRow({
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
  const mono = hit.role === "toolInput";
  return (
    <Command.Item
      value={value}
      onSelect={onOpen}
      disabled={disabled}
      data-testid="search-hit"
      className="group flex cursor-default items-baseline gap-2.5 py-[3px] pr-4 pl-8 data-[disabled=true]:opacity-50 data-[selected=true]:bg-selection"
    >
      <span className="w-[60px] shrink-0 truncate text-[12px] text-secondary">{hitRole(hit)}</span>
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

/** The key hints along the foot, for the kind of row selected (`p:`, `s:`, `h:`). */
export function SearchFooter({ kind, scoped }: { kind: string; scoped: boolean }) {
  return (
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
      {scoped ? (
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
  );
}
