import { Bug, Copy, Ellipsis, FolderOpen, Info } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { connFor, ConnectButton, lsEntryFor, STATE_WORD, useConnect, useConnectDebugInfo, useUnindexed } from "@/features/connect";
import { useLeadingInset } from "@/features/layout";
import { api } from "@/ipc";
import type { LiveState, ModelCost } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCost, formatDuration, formatRelative, formatTokens } from "@/lib/format";
import { useSession } from "@/queries";
import { useUi } from "@/state/ui";

/** Hover text of the cost: per model (main + Subagents), then what was left out. */
function costBreakdown(costs: ModelCost[], unpriced: number): string {
  const lines = costs.map((m) => `${m.model}  ${m.unpricedMessages > 0 ? "无价格" : formatCost(m.cost)}`);
  if (unpriced > 0) lines.push(`${unpriced} 条消息的模型无价格，未计入`);
  return ["Cost：按 API 单价估算（含 Subagent）", ...lines].join("\n");
}

export function SessionHeader() {
  const sessionId = useUi((s) => s.sessionId);
  const inset = useLeadingInset();
  const { data: d, error } = useSession(sessionId);
  const connected = useConnect((s) => connFor(s, sessionId));
  const lsCwd = useConnect((s) => lsEntryFor(s, sessionId)?.cwd ?? null);
  // A Connected or `ccrpc ls` Session not in the index yet (a new one, before its first prompt).
  const unindexed = useUnindexed(error);
  if (!sessionId) return null;
  if (!d) {
    if (!unindexed) return null;
    const cwd = connected?.cwd ?? lsCwd;
    return (
      <header data-testid="header" data-tauri-drag-region="deep" className={cn("flex shrink-0 flex-col gap-1.5 border-b border-border py-2.5 pr-5 pl-6", inset)}>
        <div className="flex flex-wrap items-center gap-2.5">
          <h1 className="m-0 min-w-60 flex-1 truncate text-[15px] font-[650]">新 Session</h1>
          <ConnectButton sessionId={sessionId} />
        </div>
        <div className="flex flex-wrap items-center gap-x-3.5 gap-y-0.5 text-[12px] text-secondary">
          <StateWord sessionId={sessionId} live={null} lastActiveMs={null} />
          {cwd && <span className="truncate">{cwd}</span>}
        </div>
      </header>
    );
  }

  const s = d.summary;
  const t = s.tokens;
  const project = d.projectPath.split("/").filter(Boolean).pop() ?? d.projectPath;

  return (
    <header data-testid="header" data-tauri-drag-region="deep" className={cn("flex shrink-0 flex-col gap-1.5 border-b border-border py-2.5 pr-5 pl-6", inset)}>
      <div className="flex flex-wrap items-center gap-2.5">
        <h1 className="m-0 min-w-60 flex-1 truncate text-[15px] font-[650]">{s.title}</h1>
        <ConnectButton sessionId={sessionId} />
        <MoreMenu sessionId={sessionId} resumeCommand={d.resumeCommand} />
      </div>
      <div className="flex flex-wrap items-center gap-x-3.5 gap-y-0.5 text-[12px] text-secondary">
        <StateWord sessionId={sessionId} live={s.live} lastActiveMs={s.lastActiveMs} />
        <span className="text-text/80">
          {project}
          {s.gitBranch && <span className="text-secondary"> · {s.gitBranch}</span>}
        </span>
        <span data-testid="session-cost" title={costBreakdown(d.modelCosts, s.unpricedMessages)}>
          Cost {formatCost(s.cost)}
          {s.unpricedMessages > 0 && "+"}
        </span>
        <span>{formatDuration(d.durationMs)}</span>
        <span>{s.messageCount} 条消息</span>
        <span className="group/info relative flex items-center">
          <button type="button" aria-label="更多统计" className="flex items-center rounded text-secondary outline-none hover:text-text focus-visible:outline-2 focus-visible:outline-accent">
            <Info size={13} strokeWidth={1.7} aria-hidden />
          </button>
          <span
            role="tooltip"
            data-testid="session-details"
            className="invisible absolute top-full left-0 z-20 mt-1.5 grid w-max grid-cols-[auto_auto] gap-x-4 gap-y-1 rounded-lg tabular-nums border border-border bg-ground px-3 py-2 text-[12px] shadow-lg group-focus-within/info:visible group-hover/info:visible"
          >
            <span>输出</span>
            <span className="text-right text-text">{formatTokens(t.output)}</span>
            {/* Two rows, no brackets: a full-width bracket's built-in space throws off the right-aligned column. */}
            <span>输入</span>
            <span className="text-right text-text">{formatTokens(t.input + t.cacheRead + t.cacheCreation)}</span>
            <span>缓存读取</span>
            <span className="text-right text-text">{formatTokens(t.cacheRead)}</span>
            <span>工具调用</span>
            <span className="text-right text-text">{s.toolCallCount} 次</span>
            {s.subagentCount > 0 && (
              <>
                <span>Subagent</span>
                <span className="text-right text-text">{s.subagentCount} 个</span>
              </>
            )}
            {s.primaryModel && (
              <>
                <span>模型</span>
                <span className="text-right text-text">{s.primaryModel}</span>
              </>
            )}
          </span>
        </span>
      </div>
    </header>
  );
}

/**
 * The Session's state in one word, after a dot while its process runs: the connection's state while
 * connected (ccrpc sees the turn through hooks), else the Live one; 「已结束 · 3 分钟前」 once it ended.
 */
function StateWord({ sessionId, live, lastActiveMs }: { sessionId: string; live: LiveState | null; lastActiveMs: number | null }) {
  const c = useConnect((st) => connFor(st, sessionId));
  const connected = c && c.ready && !c.procExit && !c.sessionExited ? c : null;
  const status = connected ? connected.state : (live?.status ?? null);
  if (!status) {
    return (
      <span data-testid="live-state">{lastActiveMs != null ? `已结束 · ${formatRelative(lastActiveMs)}` : "已结束"}</span>
    );
  }
  const waiting = status === "waiting";
  return (
    <span data-testid="live-state" className={cn("flex items-center gap-1.5 font-semibold", waiting ? "text-live-waiting" : "text-live")}>
      <span className={cn("size-2 rounded-full", waiting ? "bg-live-waiting" : status === "busy" ? "bg-live" : "box-border border-[1.5px] border-live")} />
      <span data-testid={connected ? "connect-state" : undefined} data-state={status}>
        {STATE_WORD[status] ?? status}
      </span>
    </span>
  );
}

/** 「···」: what used to crowd the title row, plus the connection's debug info while connected. */
function MoreMenu({ sessionId, resumeCommand }: { sessionId: string; resumeCommand: string }) {
  const showHidden = useUi((s) => s.showHidden);
  const setShowHidden = useUi((s) => s.setShowHidden);
  const debug = useConnectDebugInfo(sessionId);
  return (
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
        <DropdownMenuCheckboxItem checked={showHidden} onCheckedChange={(v) => setShowHidden(v === true)}>
          显示系统消息
        </DropdownMenuCheckboxItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void api.copyText(resumeCommand)}>
          <Copy size={13} strokeWidth={1.5} aria-hidden />
          复制 Resume 命令
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void api.copyText(sessionId)}>
          <Copy size={13} strokeWidth={1.5} aria-hidden />
          复制 Session ID
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void api.revealSessionFile(sessionId, null)}>
          <FolderOpen size={13} strokeWidth={1.5} aria-hidden />
          在 Finder 中显示
        </DropdownMenuItem>
        {debug && (
          <DropdownMenuItem onSelect={() => void api.copyText(debug)}>
            <Bug size={13} strokeWidth={1.5} aria-hidden />
            复制排查信息
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
