import { Plus } from "lucide-react";
import { cn } from "@/lib/cn";
import type { LsEntry } from "@/protocol";
import { connOf, useStore, waitingKeys } from "@/state/store";

const base = (p: string | null) => (p ? (p.replace(/\/+$/, "").split("/").pop() ?? p) : "");

interface Row {
  key: string;
  title: string;
  cwd: string | null;
  status: string | null;
  plugin: boolean;
  waiting: boolean;
  unread: boolean;
  connected: boolean;
}

/** All clay Sessions (`ccrpc ls`), waiting ones first; connections not listed yet (a new one). */
export function Sidebar() {
  const ls = useStore((s) => s.ls);
  const conns = useStore((s) => s.conns);
  const selected = useStore((s) => s.selected);
  const select = useStore((s) => s.select);
  const newSession = useStore((s) => s.newSession);
  const waiting = new Set(waitingKeys({ ls, conns }));

  const rows: Row[] = ls.map((e: LsEntry) => {
    const c = connOf({ conns }, e.holder_id);
    return {
      key: e.holder_id,
      title: e.title || base(e.cwd) || e.holder_id,
      cwd: e.cwd,
      status: c && !c.procExit ? c.state : e.status,
      plugin: e.plugin,
      waiting: waiting.has(e.holder_id),
      unread: !!c?.unread,
      connected: !!c && !c.procExit,
    };
  });
  for (const c of Object.values(conns)) {
    if (c.procExit || (c.holderId && ls.some((e) => e.holder_id === c.holderId))) continue;
    rows.push({
      key: c.holderId ?? c.conn,
      title: base(c.cwd) || "新 Session",
      cwd: c.cwd,
      status: c.state,
      plugin: true,
      waiting: c.requests.length > 0,
      unread: c.unread,
      connected: true,
    });
  }
  rows.sort((a, b) => Number(b.waiting) - Number(a.waiting));

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-border bg-sidebar" data-testid="sidebar">
      <div className="flex h-[38px] shrink-0 items-center justify-end px-2" data-tauri-drag-region>
        <button
          type="button"
          onClick={() => void newSession()}
          className="flex h-6 items-center gap-1 rounded px-2 text-[12px] text-secondary hover:bg-selection hover:text-text"
          data-testid="new-session"
        >
          <Plus size={13} aria-hidden /> 新建
        </button>
      </div>
      <ul className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
        {rows.length === 0 && <li className="px-2 py-4 text-[12px] text-secondary">本机 clay 里没有在跑的 claude</li>}
        {rows.map((r) => (
          <li key={r.key}>
            <button
              type="button"
              disabled={!r.plugin && !r.connected}
              onClick={() => void select(r.key)}
              title={r.plugin ? (r.cwd ?? "") : "这个 claude 没加载 ccrpc 插件：重启它才能连"}
              data-testid="session-row"
              data-key={r.key}
              data-waiting={r.waiting || undefined}
              className={cn(
                "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left",
                selected === r.key ? "bg-selection" : "hover:bg-black/5 dark:hover:bg-white/5",
                !r.plugin && !r.connected && "opacity-50",
              )}
            >
              <StatusDot status={r.status} waiting={r.waiting} />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[13px] text-text">{r.title}</span>
                <span className="block truncate text-[11px] text-secondary">
                  {r.plugin ? base(r.cwd) : "未加载插件"}
                </span>
              </span>
              {r.unread && <span className="mt-1.5 size-1.5 shrink-0 rounded-full bg-accent" data-testid="unread" />}
            </button>
          </li>
        ))}
      </ul>
    </aside>
  );
}

function StatusDot({ status, waiting }: { status: string | null; waiting: boolean }) {
  const cls = waiting
    ? "bg-live-waiting"
    : status === "busy"
      ? "bg-live animate-pulse"
      : status === "idle"
        ? "border border-live"
        : "border border-secondary";
  return <span className={cn("mt-1 size-2 shrink-0 rounded-full", cls)} aria-label={waiting ? "等待作答" : (status ?? "")} />;
}
