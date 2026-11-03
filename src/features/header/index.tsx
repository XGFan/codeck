import { ChevronDown, MoreHorizontal } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { Tasks } from "@/features/tasks";
import { cn } from "@/lib/cn";
import type { ConnState } from "@/state/conn";
import { useStore } from "@/state/store";

const MODES: { id: string; label: string }[] = [
  { id: "manual", label: "手动审批" },
  { id: "accept_edits", label: "自动接受编辑" },
  { id: "plan", label: "计划模式" },
  { id: "auto", label: "自动模式" },
];

const kTokens = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1)}k` : String(n));

const STATE_LABEL: Record<string, string> = {
  starting: "启动中",
  idle: "空闲",
  busy: "忙",
  waiting: "等待作答",
  exited: "已结束",
};

const menuItem =
  "flex cursor-default items-center rounded px-2 py-1 text-[12px] outline-none data-[highlighted]:bg-selection data-[disabled]:opacity-40";
const menuBox = "z-50 min-w-40 rounded-md border border-border bg-ground p-1 shadow-lg";

export function Header({ c }: { c: ConnState }) {
  const entry = useStore((s) => s.ls.find((e) => e.holder_id === c.holderId));
  // `ready` may come before claude has a cwd: `ls` has it once the Process File is written.
  const cwd = c.cwd ?? entry?.cwd ?? null;
  const title = entry?.title || (cwd ? cwd.replace(/\/+$/, "").split("/").pop() : null);
  const command = useStore((s) => s.command);
  const reconnect = useStore((s) => s.reconnect);
  const setError = useStore((s) => s.setError);
  const live = !c.procExit && !c.sessionExited;
  const run = async (msg: Record<string, unknown>) => {
    const r = await command(c.conn, msg);
    if (!r.success) setError(`${String(msg.type)} 失败：${r.error?.message ?? ""}`);
  };
  const mode = MODES.find((m) => m.id === c.mode);
  return (
    <header className="flex h-[38px] shrink-0 items-center gap-3 border-b border-border px-4" data-tauri-drag-region data-testid="header">
      <div className="min-w-0 flex-1" data-tauri-drag-region>
        <span className="truncate text-[13px] font-medium text-text">{title || c.holderId || "新 Session"}</span>
        {cwd && <span className="ml-2 truncate text-[11px] text-secondary">{cwd}</span>}
      </div>
      <span className="text-[11px] text-secondary" data-testid="state">
        {STATE_LABEL[c.state] ?? c.state}
      </span>
      {c.model && <span className="text-[11px] text-secondary">{c.model}</span>}
      {c.contextTokens != null && (
        <span className="text-[11px] text-secondary" title="上下文 token">
          {kTokens(c.contextTokens)}
        </span>
      )}
      <DropdownMenu.Root>
        <DropdownMenu.Trigger
          disabled={!live || !(c.state === "idle" || c.state === "busy")}
          className="flex h-6 items-center gap-1 rounded px-2 text-[12px] text-secondary hover:bg-selection disabled:opacity-40"
          data-testid="mode"
        >
          {mode?.label ?? c.mode ?? "模式"} <ChevronDown size={12} aria-hidden />
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content align="end" className={menuBox}>
            {MODES.map((m) => (
              <DropdownMenu.Item key={m.id} className={cn(menuItem, m.id === c.mode && "font-semibold")} onSelect={() => void run({ type: "set_mode", mode: m.id })}>
                {m.label}
              </DropdownMenu.Item>
            ))}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
      <Tasks c={c} />
      <DropdownMenu.Root>
        <DropdownMenu.Trigger className="flex size-6 items-center justify-center rounded text-secondary hover:bg-selection" aria-label="更多" data-testid="session-menu">
          <MoreHorizontal size={14} aria-hidden />
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content align="end" className={menuBox}>
            <DropdownMenu.Item className={menuItem} onSelect={() => void reconnect(c.conn)}>
              重连 ccrpc
            </DropdownMenu.Item>
            <DropdownMenu.Item className={menuItem} disabled={!live || c.state !== "idle"} onSelect={() => void run({ type: "exit" })}>
              退出 claude
            </DropdownMenu.Item>
            <DropdownMenu.Item
              className={menuItem}
              disabled={c.procExit != null || !(c.state === "idle" || c.sessionExited)}
              onSelect={() => void run({ type: "exit", close_pane: true, stop_tasks: true })}
            >
              退出并关闭 Pane
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </header>
  );
}
