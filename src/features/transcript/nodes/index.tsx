import type { Node } from "@/ipc/bindings";
import { ChevronDown, ChevronRight } from "lucide-react";
import { cn } from "@/lib/cn";
import { useUi } from "@/state/ui";
import { elapsed } from "../util";
import type { Row } from "../grouping";
import { FinalResultCard, OrphanHeader, ParallelAgents, SubagentCard, TaskPromptCard, WorkflowCard } from "./Agents";
import { Anchor } from "./Anchor";
import { AskRow } from "./Ask";
import { AssistantRow, AssistantSegment } from "./Assistant";
import { InheritedBanner, InheritedDivider } from "./Fork";
import { AttachmentLine, CompactRow, CompactSummaryCard, SystemLine, UnknownJson } from "./Misc";
import { NotificationRow, UserPromptView } from "./User";

export { TranscriptContext, scrollToNode, type ScrollRequest, type TranscriptCtx } from "./scroll";

/** Rows that continue an assistant run sit closer to the previous row. */
export const isContinuation = (row: Row) => "role" in row && row.role === null;

/** Renders one virtual-list row produced by `buildRows`. */
export function RowView({ row }: { row: Row }) {
  switch (row.kind) {
    case "prompt":
      return (
        <div data-node-id={row.node.id} data-kind="userPrompt" className={cn(row.node.hidden && "opacity-60")}>
          <UserPromptView node={row.node} body={row.body} branch={row.branch} />
        </div>
      );
    case "notification":
      return <NotificationRow node={row.node} body={row.body} />;
    case "taskPrompt":
      return <TaskPromptCard node={row.node} text={row.text} />;
    case "text":
    case "thinking":
    case "tool":
    case "group":
    case "unknownBlock":
      return <AssistantRow row={row} />;
    case "subagent":
      return (
        <AssistantSegment role={row.role}>
          <SubagentCard run={row.run} call={row.call} node={row.node} />
        </AssistantSegment>
      );
    case "parallel":
      return (
        <AssistantSegment role={row.role}>
          <ParallelAgents cards={row.cards} />
        </AssistantSegment>
      );
    case "workflow":
      return (
        <AssistantSegment role={row.role}>
          <WorkflowCard run={row.run} node={row.node} call={row.call} />
        </AssistantSegment>
      );
    case "ask":
      return <AskRow node={row.node} call={row.call} role={row.role} />;
    case "compact":
      return <CompactRow boundary={row.boundary} summary={row.summary} />;
    case "node":
      return <NodeView node={row.node} />;
    case "inheritedBanner":
      return (
        <InheritedBanner
          range={row.range}
          count={row.count}
          expanded={row.expanded}
          lastTimestampMs={row.lastTimestampMs}
        />
      );
    case "inheritedDivider":
      return <InheritedDivider />;
    case "orphanHeader":
      return <OrphanHeader count={row.count} />;
    case "orphan":
      return <SubagentCard run={row.run} />;
    case "finalResult":
      return <FinalResultCard text={row.text} />;
    case "fold":
      return <FoldRow row={row} />;
  }
}

/** A turn's folded process (Codex style): how long it took and what it holds, 「已处理 1 分 50 秒 · …」; a click opens it. */
function FoldRow({ row }: { row: Extract<Row, { kind: "fold" }> }) {
  const parts = [row.texts && `${row.texts} 段说明`, row.calls && `${row.calls} 次工具调用`].filter(Boolean).join("、");
  return (
    <button
      type="button"
      data-testid="turn-fold"
      aria-expanded={row.open}
      onClick={() => useUi.getState().setExpanded(row.key, !row.open)}
      className="flex min-h-7 w-full items-center gap-2 text-[13px] text-secondary hover:text-text"
    >
      {row.open ? (
        <ChevronDown size={13} strokeWidth={1.8} className="shrink-0" aria-hidden />
      ) : (
        <ChevronRight size={13} strokeWidth={1.8} className="shrink-0" aria-hidden />
      )}
      <span className="shrink-0">{row.ms != null ? `已处理 ${elapsed(row.ms)}` : "过程"}</span>
      {parts && <span className="shrink-0 text-secondary/80">· {parts}</span>}
      <span className="h-px flex-1 bg-border" />
    </button>
  );
}

/** System / attachment / unknown entries, non-turn prompts and standalone compact summaries. */
export function NodeView({ node }: { node: Node }) {
  const b = node.body;
  return (
    <Anchor nodeId={node.id}>
      <div data-node-id={node.id} data-kind={b.kind} className={cn(node.hidden && "opacity-60")}>
        {b.kind === "userPrompt" && <UserPromptView node={node} body={b} />}
        {b.kind === "compactSummary" && <CompactSummaryCard node={node} body={b} />}
        {b.kind === "system" && <SystemLine body={b} />}
        {b.kind === "attachment" && <AttachmentLine body={b} />}
        {b.kind === "unknown" && <UnknownJson label={`未知记录 · ${b.entryType}`} json={b.rawJson} />}
      </div>
    </Anchor>
  );
}
