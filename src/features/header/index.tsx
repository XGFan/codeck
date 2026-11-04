import { Copy, Ellipsis, FolderOpen } from "lucide-react";
import { Checkbox } from "@/components/ui/checkbox";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { connFor, DriveBar, DriveConnect, lsEntryFor, useDrive, useUnindexed } from "@/features/drive";
import { useLeadingInset } from "@/features/layout";
import { api } from "@/ipc";
import type { ModelCost } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCost, formatDuration, formatRelative, formatTokens } from "@/lib/format";
import { useSession } from "@/queries";
import { useUi } from "@/state/ui";

/** Hover text of the cost: per model (main + Subagents), then what was left out. */
function costBreakdown(costs: ModelCost[], unpriced: number): string {
  const lines = costs.map((m) => `${m.model}  ${m.unpricedMessages > 0 ? "无价格" : formatCost(m.cost)}`);
  if (unpriced > 0) lines.push(`${unpriced} 条消息的模型无价格，未计入`);
  return ["按 API 价格估算（含 Subagent）", ...lines].join("\n");
}

export function SessionHeader() {
  const sessionId = useUi((s) => s.sessionId);
  const showHidden = useUi((s) => s.showHidden);
  const setShowHidden = useUi((s) => s.setShowHidden);
  const inset = useLeadingInset();
  const { data: d, error } = useSession(sessionId);
  const driven = useDrive((s) => connFor(s, sessionId));
  const lsCwd = useDrive((s) => lsEntryFor(s, sessionId)?.cwd ?? null);
  // A driven or `ccrpc ls` Session not in the index yet (a new one, before its first prompt).
  const unindexed = useUnindexed(error);
  if (!sessionId) return null;
  if (!d) {
    if (!unindexed) return null;
    const cwd = driven?.cwd ?? lsCwd;
    return (
      <header data-testid="header" data-tauri-drag-region="deep" className={cn("flex shrink-0 flex-col gap-1.5 border-b border-border py-2.5 pr-5 pl-6", inset)}>
        <div className="flex flex-wrap items-center gap-2.5">
          <h1 className="m-0 min-w-60 flex-1 truncate text-[15px] font-[650]">新 Session</h1>
          <DriveConnect sessionId={sessionId} />
        </div>
        {cwd && <div className="truncate text-[12px] text-secondary">{cwd}</div>}
        <DriveBar sessionId={sessionId} />
      </header>
    );
  }

  const s = d.summary;
  const live = s.live;
  const t = s.tokens;
  const project = d.projectPath.split("/").filter(Boolean).pop() ?? d.projectPath;
  const liveText = live
    ? `进行中 · ${live.status === "busy" ? "工作中" : live.status === "waiting" ? "等待作答" : "等待输入"}`
    : `已结束 · ${formatRelative(s.lastActiveMs)}`;

  return (
    <header data-testid="header" data-tauri-drag-region="deep" className={cn("flex shrink-0 flex-col gap-1.5 border-b border-border py-2.5 pr-5 pl-6", inset)}>
      <div className="flex flex-wrap items-center gap-2.5">
        <h1 className="m-0 min-w-60 flex-1 truncate text-[15px] font-[650]">{s.title}</h1>
        <DriveConnect sessionId={sessionId} />
        <label className="flex items-center gap-1.5 text-[12px] text-text/80">
          <Checkbox checked={showHidden} onCheckedChange={(v) => setShowHidden(v === true)} />
          显示系统消息
        </label>
        <button
          type="button"
          onClick={() => void api.copyText(d.resumeCommand)}
          className="flex h-7 items-center gap-1.5 rounded-md border border-border bg-ground px-2.5 text-[12px] hover:bg-selection"
        >
          <Copy size={13} strokeWidth={1.5} aria-hidden />
          复制 resume 命令
        </button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              data-testid="header-more"
              aria-label="更多操作"
              className="flex size-7 items-center justify-center rounded-md border border-border bg-ground hover:bg-selection"
            >
              <Ellipsis size={14} strokeWidth={1.6} aria-hidden />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => void api.copyText(d.summary.id)}>
              <Copy size={13} strokeWidth={1.5} aria-hidden />
              复制 Session ID
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void api.revealSessionFile(d.summary.id, null)}>
              <FolderOpen size={13} strokeWidth={1.5} aria-hidden />
              在 Finder 中显示
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <div className="flex flex-wrap items-center gap-x-3.5 gap-y-0.5 text-[12px] text-secondary">
        <span data-testid="live-state" className={cn("flex items-center gap-1.5", live && "font-semibold text-live")}>
          {live && <span className="size-2 rounded-full bg-live" />}
          {liveText}
        </span>
        <span className="text-text/80">{project}</span>
        {s.gitBranch && <span>{s.gitBranch}</span>}
        {s.primaryModel && <span>{s.primaryModel}</span>}
        <span>
          输出 {formatTokens(t.output)} · 输入 {formatTokens(t.input + t.cacheRead + t.cacheCreation)}（缓存 {formatTokens(t.cacheRead)}）
        </span>
        <span data-testid="session-cost" title={costBreakdown(d.modelCosts, s.unpricedMessages)}>
          API {formatCost(s.cost)}
          {s.unpricedMessages > 0 && "+"}
        </span>
        <span>{formatDuration(d.durationMs)}</span>
        <span>{s.messageCount} 条消息</span>
        <span>{s.toolCallCount} 次工具调用</span>
        {s.subagentCount > 0 && <span>{s.subagentCount} 个 Subagent</span>}
      </div>
      <DriveBar sessionId={sessionId} />
    </header>
  );
}
