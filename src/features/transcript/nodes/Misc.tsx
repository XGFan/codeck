import { ChevronDown, ChevronRight, Minimize2 } from "lucide-react";
import { Markdown } from "@/lib/markdown";
import { cn } from "@/lib/cn";
import type { Node } from "@/ipc/bindings";
import { useUi } from "@/state/ui";
import { formatTokens } from "@/lib/format";
import { compactKey } from "../keys";
import { clock, prettyJson } from "../util";
import { Anchor } from "./Anchor";

type Body<K extends Node["body"]["kind"]> = Extract<Node["body"], { kind: K }>;

export function SystemLine({ body }: { body: Body<"system"> }) {
  const warn = body.level === "error" || body.level === "warning";
  return (
    <div
      className={cn("flex items-baseline gap-2 text-[12px]", body.level === "error" ? "text-error" : "text-secondary")}
    >
      <span className="shrink-0 rounded border border-border px-1 text-[11px]">{warn ? "警告" : "系统"}</span>
      <span className="min-w-0 break-words whitespace-pre-wrap">{body.text}</span>
    </div>
  );
}

export function AttachmentLine({ body }: { body: Body<"attachment"> }) {
  return (
    <div className="flex items-baseline gap-2 text-[12px] text-secondary">
      <span className="shrink-0 rounded border border-border px-1 text-[11px]">附件</span>
      <span className="min-w-0 break-words">{body.text || body.attachmentType}</span>
    </div>
  );
}

/** Compact boundary divider; when the summary follows it, "查看摘要" expands it below the line. */
export function CompactRow({ boundary, summary }: { boundary: Node; summary: Node | null }) {
  const key = summary ? compactKey(summary.id) : "";
  const open = useUi((s) => (summary ? (s.expanded[key] ?? false) : false));
  const toggle = useUi((s) => s.toggleExpanded);
  const pre = boundary.body.kind === "compactBoundary" ? boundary.body.preTokens : null;
  return (
    <Anchor nodeId={boundary.id} className="flex flex-col gap-2">
      <div data-testid="compact-divider" className="flex items-center gap-2.5 text-[12px] text-secondary">
        <span className="h-px flex-1 bg-border" />
        <Minimize2 size={13} strokeWidth={1.5} aria-hidden />
        <span>
          上下文已压缩 · {clock(boundary.timestampMs)}
          {pre != null && <span> · 压缩前 {formatTokens(pre)}</span>}
        </span>
        {summary && (
          <button
            type="button"
            aria-expanded={open}
            onClick={() => toggle(key)}
            className="text-accent hover:underline"
          >
            {open ? "收起摘要" : "查看摘要"}
          </button>
        )}
        <span className="h-px flex-1 bg-border" />
      </div>
      {summary && open && summary.body.kind === "compactSummary" && (
        <Anchor nodeId={summary.id}>
          <Markdown
            text={summary.body.text}
            className="rounded-lg border border-border bg-list px-3 py-2 text-[12px]"
          />
        </Anchor>
      )}
    </Anchor>
  );
}

export function CompactSummaryCard({ node, body }: { node: Node; body: Body<"compactSummary"> }) {
  const key = compactKey(node.id);
  const open = useUi((s) => s.expanded[key] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  return (
    <Anchor nodeId={node.id} className="overflow-hidden rounded-lg border border-border">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => toggle(key)}
        className="flex w-full items-center gap-2 bg-list px-2.5 py-1.5 text-left text-[12px] text-text/80"
      >
        {open ? <ChevronDown size={12} strokeWidth={1.8} /> : <ChevronRight size={12} strokeWidth={1.8} />}
        <span className="font-semibold">上下文摘要</span>
      </button>
      {open && <Markdown text={body.text} className="border-t border-border px-3 py-2 text-[12px]" />}
    </Anchor>
  );
}

export function UnknownJson({ label, json }: { label: string; json: string }) {
  return (
    <div className="w-full overflow-hidden rounded-lg border border-border">
      <div className="bg-list px-2.5 py-1 text-[11px] text-secondary">{label}</div>
      <pre className="m-0 max-h-60 overflow-auto bg-code px-3 py-2 font-mono text-[12px] leading-[1.55] whitespace-pre-wrap break-words">
        {prettyJson(json)}
      </pre>
    </div>
  );
}
