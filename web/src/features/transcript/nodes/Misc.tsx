import { ChevronDown, ChevronRight, Minimize2, TriangleAlert } from "lucide-react";
import { Markdown } from "@/lib/markdown";
import { cn } from "@/lib/cn";
import type { Node } from "@/ipc/bindings";
import { useUi } from "@/state/ui";
import { formatDuration, formatTokens } from "@/lib/format";
import { compactKey } from "../keys";
import { clock, prettyJson } from "../util";
import { Anchor } from "./Anchor";

type Body<K extends Node["body"]["kind"]> = Extract<Node["body"], { kind: K }>;

/** Wording of a system entry that carries nothing beyond its envelope. */
function emptySystemText(subtype: string): string {
  if (subtype === "agents_killed") return "已终止所有后台 agent";
  return subtype || "未知系统记录";
}

export function SystemLine({ body }: { body: Body<"system"> }) {
  const warn = body.level === "error" || body.level === "warning";
  return (
    <div
      className={cn("flex items-baseline gap-2 text-[12px]", body.level === "error" ? "text-error" : "text-secondary")}
    >
      <span className="shrink-0 rounded border border-border px-1 text-[11px]">{warn ? "警告" : "系统"}</span>
      <span className="min-w-0 break-words whitespace-pre-wrap">{body.text || emptySystemText(body.subtype)}</span>
    </div>
  );
}

/** A tool result whose call is not on the shown path. */
export function ToolResultLine({ body }: { body: Body<"toolResult"> }) {
  return (
    <div className="flex items-baseline gap-2 text-[12px] text-secondary">
      <span className="shrink-0 rounded border border-border px-1 text-[11px]">工具结果</span>
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

/** What one API retry says (`system/api_error`, shown as its JSON fields); null when its text is not that JSON. */
export function retryInfo(node: Node): { error: string; attempt: number; max: number; waitMs: number | null } | null {
  const b = node.body;
  if (b.kind !== "system") return null;
  const v = parseJsonObject(b.text);
  if (!v) return null;
  const e = (v.error ?? {}) as Record<string, unknown>;
  const conn = (e.connection ?? null) as Record<string, unknown> | null;
  const error =
    (typeof e.formatted === "string" && e.formatted) ||
    (conn && typeof conn.code === "string" && conn.code) ||
    (typeof e.message === "string" && e.message.split("\n", 1)[0]) ||
    "";
  const attempt = typeof v.retryAttempt === "number" ? v.retryAttempt : 0;
  const max = typeof v.maxRetries === "number" ? v.maxRetries : 0;
  return { error, attempt, max, waitMs: typeof v.retryInMs === "number" ? v.retryInMs : null };
}

function parseJsonObject(text: string): Record<string, unknown> | null {
  try {
    const v: unknown = JSON.parse(text);
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/**
 * Consecutive API retries as one line — the error and how many — red, or grey when Claude went on after them;
 * opened, each attempt and how long Claude Code waited before it.
 */
export function ApiRetryRow({ nodes, recovered, stateKey }: { nodes: Node[]; recovered: boolean; stateKey: string }) {
  const open = useUi((s) => s.expanded[stateKey] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const infos = nodes.map(retryInfo);
  const error = infos.find((i) => i?.error)?.error ?? "";
  const what = error ? `API 请求失败：${error}` : "API 请求失败";
  return (
    <Anchor nodeId={nodes[0]!.id} className="flex flex-col gap-1">
      <button
        type="button"
        data-testid="api-retry"
        data-recovered={recovered || undefined}
        aria-expanded={open}
        onClick={() => toggle(stateKey)}
        className={cn(
          "flex items-center gap-2 text-left text-[12px] hover:underline",
          recovered ? "text-secondary" : "text-error",
        )}
      >
        {open ? (
          <ChevronDown size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
        ) : (
          <ChevronRight size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
        )}
        <TriangleAlert size={13} strokeWidth={1.8} className="shrink-0" aria-hidden />
        <span className="min-w-0">
          {what}
          {recovered ? `，重试 ${nodes.length} 次后恢复` : ` · 重试 ${nodes.length} 次`}
        </span>
        <span className="shrink-0 text-secondary">{clock(nodes[0]!.timestampMs)}</span>
      </button>
      {open && (
        <div
          data-testid="api-retry-list"
          className="ml-1.5 flex flex-col gap-0.5 border-l-2 border-border pl-4 text-[12px] text-secondary"
        >
          {nodes.map((n, i) => {
            const r = infos[i];
            const line = r
              ? [
                  r.max ? `第 ${r.attempt}/${r.max} 次` : `第 ${r.attempt || i + 1} 次`,
                  r.waitMs != null ? `${formatDuration(r.waitMs, "detail")}后重试` : null,
                  clock(n.timestampMs),
                ]
                  .filter(Boolean)
                  .join(" · ")
              : n.body.kind === "system" && n.body.text;
            // The first one's anchor is the row itself.
            return i === 0 ? (
              <div key={n.id}>{line}</div>
            ) : (
              <Anchor key={n.id} nodeId={n.id}>
                {line}
              </Anchor>
            );
          })}
        </div>
      )}
    </Anchor>
  );
}
