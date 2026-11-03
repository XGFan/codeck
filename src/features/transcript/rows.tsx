import { ChevronDown, ChevronRight, CircleX, Network, Workflow } from "lucide-react";
import { CopyButton } from "@/components/ui/copy-button";
import { cn } from "@/lib/cn";
import { formatTokens } from "@/lib/format";
import { Markdown } from "@/lib/markdown";
import type { TurnEndEvent } from "@/protocol";
import { useStore } from "@/state/store";
import { useUi } from "@/state/ui";
import type { GroupItem, Row, ToolCall, ToolResult } from "./model";
import { ToolCallView } from "./tools";
import { expandKey, str, toolKey, useExpanded } from "./tools/data";
import { StatusBadge } from "./tools/parts";

/**
 * Rows of one block run. A role line ("Claude") opens each assistant run; `userLabel` names the user side
 * ("你" on the Main Line, "任务" in a Subagent Run).
 */
export function RowList({ conn, rows, userLabel = "你" }: { conn: string; rows: Row[]; userLabel?: string }) {
  return (
    <>
      {rows.map((row, i) => {
        const prev = rows[i - 1];
        const assistant = row.kind !== "user" && row.kind !== "turn_end";
        const role = assistant && (!prev || prev.kind === "user");
        return (
          <div key={row.key} data-row={row.kind} data-row-key={row.key} className="flex w-full flex-col items-start gap-2">
            {role && <RoleLine />}
            <RowView conn={conn} row={row} userLabel={userLabel} />
          </div>
        );
      })}
    </>
  );
}

function RoleLine() {
  return (
    <div className="flex items-center gap-1.5 text-[11px] text-secondary">
      <span className="flex size-4 items-center justify-center rounded bg-claude text-[10px] font-bold text-white">C</span>
      Claude
    </div>
  );
}

function RowView({ conn, row, userLabel }: { conn: string; row: Row; userLabel: string }) {
  switch (row.kind) {
    case "user":
      return <UserBubble text={row.text} queued={row.queued} label={userLabel} />;
    case "text":
      return (
        <div data-testid="text-block" className="group/copy relative w-full">
          <CopyButton text={row.text} className="absolute -top-2 right-0 z-10 bg-ground/90" />
          <Markdown text={row.text} className="w-full" />
        </div>
      );
    case "thinking":
      return <Thinking conn={conn} k={row.key} text={row.text} />;
    case "tool":
      return <ToolCallView conn={conn} call={row.call} />;
    case "group":
      return <ToolGroup conn={conn} row={row} />;
    case "agent":
      return <AgentCard conn={conn} call={row.call} />;
    case "result":
      return <OrphanResult result={row.result} />;
    case "turn_end":
      return <TurnEnd ev={row.ev} />;
  }
}

function UserBubble({ text, queued, label }: { text: string; queued: boolean; label: string }) {
  return (
    <div data-testid="user-block" data-queued={queued || undefined} className="group/copy flex w-full flex-col items-end gap-1.5">
      <div className="flex items-center gap-2 text-[11px] text-secondary">
        <CopyButton text={text} />
        {queued && <span className="rounded border border-live-waiting/40 px-1 text-[11px] leading-[16px] text-live-waiting">插话</span>}
        <span>{label}</span>
      </div>
      <div
        className={cn(
          "max-w-[640px] rounded-xl border px-3.5 py-2.5 break-words whitespace-pre-wrap",
          queued ? "border-dashed border-live-waiting/40 bg-selection/30" : "border-selection bg-selection/50",
        )}
      >
        {text}
      </div>
    </div>
  );
}

function Thinking({ conn, k, text }: { conn: string; k: string; text: string }) {
  const key = expandKey(conn, `thinking:${k}`);
  const open = useExpanded(key);
  const lines = text.split("\n").length;
  return (
    <div data-testid="thinking-block" className="w-full">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => useUi.getState().toggle(key, !open)}
        className="flex items-center gap-2 rounded px-1 py-1 text-[12px] text-secondary hover:text-text"
      >
        {open ? <ChevronDown size={12} strokeWidth={1.8} /> : <ChevronRight size={12} strokeWidth={1.8} />}
        <span className="italic">思考 · {lines} 行</span>
      </button>
      {open && <div className="mt-1 ml-1 border-l-2 border-border pl-3 text-[12px] whitespace-pre-wrap text-secondary">{text}</div>}
    </div>
  );
}

/** "N 次工具调用 · 3 Bash · 2 Read"; collapsed unless opened or holding an expanded call. */
function ToolGroup({ conn, row }: { conn: string; row: Extract<Row, { kind: "group" }> }) {
  const key = expandKey(conn, row.key);
  // A lone call that becomes a group when the next call arrives keeps the user's expansion visible.
  const open = useUi(
    (s) => s.expanded[key] ?? row.items.some((it) => it.type === "call" && s.expanded[toolKey(conn, it.call.id)] === true),
  );
  const running = row.items.some((it) => it.type === "call" && !it.call.result && !it.call.unfinished);
  return (
    <div data-testid="tool-group" data-open={open || undefined} className="flex w-full flex-col items-start gap-1.5">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => useUi.getState().toggle(key, !open)}
        className="flex max-w-full items-center gap-2 rounded-lg border border-border bg-list px-2.5 py-1.5 text-[12px] text-text/80 hover:bg-selection/40"
      >
        {open ? <ChevronDown size={12} strokeWidth={1.8} aria-hidden /> : <ChevronRight size={12} strokeWidth={1.8} aria-hidden />}
        <span className="shrink-0 font-semibold">{row.callCount} 次工具调用</span>
        <span className="min-w-0 truncate text-secondary">{row.counts.map(([name, n]) => `${n} ${name}`).join(" · ")}</span>
        {running && <span className="shrink-0 text-secondary">执行中</span>}
        {row.failedCount > 0 && (
          <span className="flex shrink-0 items-center gap-1 font-semibold text-error">
            <CircleX size={12} strokeWidth={1.8} aria-hidden />
            {row.failedCount} 失败
          </span>
        )}
      </button>
      {open && (
        <div className="flex w-full flex-col gap-1.5 border-l-2 border-border pl-3">
          {row.items.map((it) => (
            <GroupEntry key={it.type === "call" ? it.call.id : it.key} conn={conn} item={it} />
          ))}
        </div>
      )}
    </div>
  );
}

function GroupEntry({ conn, item }: { conn: string; item: GroupItem }) {
  if (item.type === "thinking") return <Thinking conn={conn} k={item.key} text={item.text} />;
  return <ToolCallView conn={conn} call={item.call} />;
}

/** Agent / Task / Workflow call: description, sub-block count, prompt and result previews, "查看过程". */
export function AgentCard({ conn, call }: { conn: string; call: ToolCall }) {
  const subCount = useStore((s) => s.conns[conn]?.sub[call.id]?.length ?? 0);
  const workflow = useStore((s) => {
    const w = s.conns[conn]?.workflows;
    return w ? Object.values(w).find((x) => x.tool_use_id === call.id) : undefined;
  });
  const active = useUi((s) => s.panel?.conn === conn && s.panel.toolUseId === call.id);
  const isWorkflow = call.name === "Workflow";
  const title = isWorkflow ? "Workflow" : str(call.input.subagent_type) || "Subagent";
  const desc = isWorkflow
    ? workflow?.name || str(call.input.name) || str(call.input.description)
    : str(call.input.description);
  const prompt = isWorkflow ? "" : str(call.input.prompt);
  const failed = call.result?.isError === true;
  const Icon = isWorkflow ? Workflow : Network;
  return (
    <div
      data-testid="tool-call"
      data-tool={call.name}
      data-active={active || undefined}
      className={cn(
        "flex w-full flex-col gap-1.5 rounded-[10px] border px-3 py-2.5",
        active ? "border-accent bg-selection/40" : "border-border bg-ground",
      )}
    >
      <div className="flex flex-wrap items-center gap-2 text-[13px]">
        <Icon size={14} strokeWidth={1.5} className={active ? "text-accent" : "text-text/80"} aria-hidden />
        <span className="font-semibold">{title}</span>
        <span className="min-w-[120px] flex-1 truncate">{desc}</span>
        {workflow && <span className="text-[12px] text-secondary">{workflow.status}</span>}
        <span className="text-[12px] text-secondary">{subCount} 块</span>
        <span className={cn("text-[12px]", failed && "text-error")}>
          <StatusBadge call={call} />
        </span>
      </div>
      {prompt && (
        <div className="line-clamp-2 text-[12px] text-text/80">
          <span className="text-secondary">任务</span>
          <span className="ml-3">{prompt}</span>
        </div>
      )}
      {call.result && call.result.content.trim() && (
        <div data-testid="tool-result" className="line-clamp-2 text-[12px] text-text/80">
          <span className={failed ? "text-error" : "text-live"}>{failed ? "失败" : "结果"}</span>
          <span className="ml-3">{call.result.content}</span>
        </div>
      )}
      <div className="flex justify-end text-[12px]">
        {active ? (
          <span className="font-semibold text-accent">正在右侧查看</span>
        ) : (
          <button
            type="button"
            data-testid="open-subagent"
            onClick={() => useUi.getState().openPanel(conn, call.id)}
            className="font-semibold text-accent hover:underline"
          >
            查看过程 →
          </button>
        )}
      </div>
    </div>
  );
}

/** A tool result whose call is on an older, unloaded page. */
function OrphanResult({ result }: { result: ToolResult }) {
  return (
    <div
      data-testid="tool-result"
      className={cn(
        "line-clamp-3 w-full rounded-lg border px-2.5 py-1.5 font-mono text-[12px] whitespace-pre-wrap",
        result.isError ? "border-error/30 text-error" : "border-border text-secondary",
      )}
    >
      {result.content || "（无输出）"}
    </div>
  );
}

/** "12.3 秒"、"1 分 04 秒"、"1 小时 12 分". */
export function durationText(ms: number): string {
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} 秒`;
  const sec = Math.round(ms / 1000);
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m} 分 ${String(sec % 60).padStart(2, "0")} 秒`;
  const h = Math.floor(m / 60);
  return m % 60 ? `${h} 小时 ${m % 60} 分` : `${h} 小时`;
}

function TurnEnd({ ev }: { ev: TurnEndEvent }) {
  const tokens = ev.usage.input_tokens + ev.usage.output_tokens;
  const parts = [
    ev.duration_ms != null ? durationText(ev.duration_ms) : null,
    tokens > 0 ? `${formatTokens(tokens)} tokens` : null,
    ev.api_cost != null ? `$${ev.api_cost.toFixed(4)}` : null,
  ].filter(Boolean);
  return (
    <div data-testid="turn-end" className="flex w-full flex-wrap items-center gap-x-3 gap-y-0.5 pt-1 text-[11px] text-secondary">
      <span className="h-px w-6 bg-border" aria-hidden />
      {parts.length > 0 && <span>{parts.join(" · ")}</span>}
      {ev.aborted && <span className="font-semibold">已中止</span>}
      {ev.error && <span className="text-error">{ev.error.message}</span>}
    </div>
  );
}
