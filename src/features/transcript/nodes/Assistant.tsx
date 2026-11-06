import { ChevronDown, ChevronRight, CircleX, Lightbulb } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import { CopyButton } from "@/components/ui/copy-button";
import { Markdown } from "@/lib/markdown";
import { useUi } from "@/state/ui";
import { BranchSwitcher } from "../BranchSwitcher";
import { describeCalls, type GroupItem, isGroupOpen, type RoleInfo, type Row, type ToolCallBlock } from "../grouping";
import { thinkingKey } from "../keys";
import { ToolCallView, verbOf } from "../tools";
import { connFor, useConnect } from "@/features/connect";
import { formatDuration } from "@/lib/format";
import { clock } from "../util";
import { Anchor } from "./Anchor";
import { UnknownJson } from "./Misc";
import { useTranscriptCtx } from "./scroll";
import type { Node } from "@/ipc/bindings";

type AssistantRowData = Extract<Row, { kind: "text" | "thinking" | "tool" | "group" | "unknownBlock" }>;

/**
 * Wraps one segment of an assistant run. No 「Claude · time」 line (the answer's time is under it); the first
 * segment shows only what needs saying: the branch switcher at a branch head, an API error.
 */
export function AssistantSegment({ role, children }: { role: RoleInfo | null; children: ReactNode }) {
  return (
    <div data-kind="assistant" className="flex flex-col items-start gap-2">
      {role && <RoleLine role={role} />}
      {children}
    </div>
  );
}

function RoleLine({ role }: { role: RoleInfo }) {
  const b = role.node.body;
  const apiError = b.kind === "assistant" && b.isApiError;
  if (!role.branch && !apiError) return null;
  return (
    <div data-testid="role-line" className="flex items-center gap-1.5 text-[11px] text-secondary">
      {role.branch && <BranchSwitcher point={role.branch} />}
      {apiError && (
        <span data-testid="api-error" className="font-semibold text-error">
          API 错误{role.retries?.length ? ` · 重试 ${role.retries.length} 次后放弃` : ""} ·{" "}
          {clock(role.node.timestampMs)}
        </span>
      )}
    </div>
  );
}

/** Under a turn's answer: copy all of it, and when it was written. */
function AnswerActions({ text, timeMs }: { text: string; timeMs: number | null }) {
  return (
    <div data-testid="answer-actions" className="-mt-1 flex items-center gap-2 text-[11.5px] text-secondary">
      <CopyButton text={text} className="opacity-100" />
      <span>{clock(timeMs)}</span>
    </div>
  );
}

export function AssistantRow({ row }: { row: AssistantRowData }) {
  return <AssistantSegment role={row.role}>{segmentBody(row)}</AssistantSegment>;
}

function segmentBody(row: AssistantRowData): ReactNode {
  switch (row.kind) {
    case "text":
      return row.text.trim() ? (
        <>
          <Anchor nodeId={row.node.id} className="group/copy relative w-full">
            {!row.answer && <CopyButton text={row.text} className="absolute -top-2 right-0 z-10 bg-ground/90" />}
            {/* The size of the input box's text. */}
            <Markdown text={row.text} className="w-full text-[14px] leading-[1.7]" />
          </Anchor>
          {row.answer && <AnswerActions {...row.answer} />}
        </>
      ) : (
        <Anchor nodeId={row.node.id} className="h-px w-full">
          {null}
        </Anchor>
      );
    case "thinking":
      return <Thinking nodeId={row.node.id} text={row.text} />;
    case "tool":
      return <Tool node={row.node} call={row.call} />;
    case "group":
      return <ToolGroup row={row} />;
    case "unknownBlock":
      return <UnknownJson label={`未知内容块 · ${row.blockType}`} json={row.rawJson} />;
  }
}

function Tool({ node, call }: { node: Node; call: ToolCallBlock }) {
  const { scope, sessionId } = useTranscriptCtx();
  return (
    <Anchor nodeId={node.id} toolUseId={call.toolUseId} className="w-full">
      <ToolCallView nodeId={node.id} call={call} startMs={node.timestampMs} sessionId={sessionId} scope={scope} />
    </Anchor>
  );
}

/**
 * One line for a run of calls: what they did (「读取 1 个文件、编辑 2 个文件、运行 1 条命令」) and on what;
 * collapsed unless the user opened it; failed calls are counted at the end.
 */
function ToolGroup({ row }: { row: Extract<Row, { kind: "group" }> }) {
  const failed = row.failedCount > 0;
  const open = useUi((s) => isGroupOpen(row, s.expanded));
  const { text, targets } = useMemo(
    () => describeCalls(row.items.flatMap((it) => (it.type === "call" ? [it.call] : []))),
    [row.items],
  );
  // In the running turn: the call still without a result (the last such), said after the summary.
  const current = useMemo(() => {
    if (!row.live) return null;
    for (let i = row.items.length - 1; i >= 0; i--) {
      const it = row.items[i]!;
      if (it.type === "call" && it.call.result == null) return it;
    }
    return null;
  }, [row]);
  return (
    <div data-testid="tool-group" data-open={open || undefined} className="flex w-full flex-col items-start gap-1.5">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => useUi.getState().setExpanded(row.key, !open)}
        className="flex min-h-7 max-w-full items-center gap-2 text-[13px] text-secondary hover:text-text"
      >
        {open ? (
          <ChevronDown size={13} strokeWidth={1.8} className="shrink-0" aria-hidden />
        ) : (
          <ChevronRight size={13} strokeWidth={1.8} className="shrink-0" aria-hidden />
        )}
        <span className="shrink-0 text-text/90">{text}</span>
        {current ? (
          <CurrentCall node={current.node} call={current.call} />
        ) : (
          <span className="min-w-0 truncate font-mono text-[12px] text-secondary">{targets.join(" · ")}</span>
        )}
        {failed && (
          <span className="flex shrink-0 items-center gap-1 text-error">
            <CircleX size={12} strokeWidth={1.8} aria-hidden />
            {row.failedCount} 失败
          </span>
        )}
      </button>
      {open && (
        <div className="ml-1.5 flex w-full flex-col gap-0.5 border-l-2 border-border pl-4">
          {row.items.map((it) => (
            <GroupEntry key={itemKey(it)} item={it} />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * What a running group does now: 「· 正在运行 pnpm test… 12 秒」, or 「· 等待审批：运行 pnpm test」 while a
 * permission card waits on it.
 */
function CurrentCall({ node, call }: { node: Node; call: ToolCallBlock }) {
  const { sessionId } = useTranscriptCtx();
  const waiting = useConnect(
    (s) => connFor(s, sessionId)?.requests.some((r) => "tool_use_id" in r && r.tool_use_id === call.toolUseId) ?? false,
  );
  const ms = useElapsed(node.timestampMs, !waiting);
  const target = describeCalls([call]).targets[0];
  const what = `${verbOf(call.name)}${target ? ` ${target}` : ""}`;
  return (
    <span data-testid="group-current" className="flex min-w-0 items-center gap-1.5 text-[12px]">
      <span aria-hidden>·</span>
      <span className="min-w-0 truncate text-live-waiting">{waiting ? `等待审批：${what}` : `正在${what}…`}</span>
      {ms != null && <span className="shrink-0 text-secondary/80">{formatDuration(ms, "detail")}</span>}
    </span>
  );
}

/** Time since `startMs`, ticking every second while `on`. */
function useElapsed(startMs: number | null, on: boolean): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [on]);
  return on && startMs != null ? Math.max(0, now - startMs) : null;
}

const itemKey = (it: GroupItem) => (it.type === "call" ? `c:${it.call.toolUseId}` : `t:${it.node.id}:${it.blockIndex}`);

function GroupEntry({ item }: { item: GroupItem }) {
  if (item.type === "thinking") return <Thinking nodeId={item.node.id} text={item.text} />;
  return <Tool node={item.node} call={item.call} />;
}

/**
 * A thinking block, collapsed to a light bulb and its line count; opened, a light block (not a rule, which is a
 * tool group's). Blank lines around the text are not shown or counted.
 */
function Thinking({ nodeId, text }: { nodeId: string; text: string }) {
  const key = thinkingKey(nodeId);
  const open = useUi((s) => s.expanded[key] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const body = text.trim();
  const lines = body.split("\n").length;
  return (
    <Anchor nodeId={nodeId} className="w-full">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => toggle(key)}
        className="flex min-h-7 items-center gap-2 text-[13px] text-secondary hover:text-text"
      >
        <Lightbulb size={14} strokeWidth={1.6} className="shrink-0" aria-hidden />
        <span>思考</span>
        <span data-testid="thinking-lines" className="text-[12px]">
          {lines} 行
        </span>
        {open ? (
          <ChevronDown size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
        ) : (
          <ChevronRight size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
        )}
      </button>
      {open && (
        <div
          data-testid="thinking-text"
          className="mt-0.5 mb-1 ml-[22px] rounded-lg bg-list px-3 py-2 text-[12.5px] leading-[1.6] whitespace-pre-wrap text-secondary"
        >
          {body}
        </div>
      )}
    </Anchor>
  );
}
