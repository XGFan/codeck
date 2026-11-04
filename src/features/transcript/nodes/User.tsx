import { ArrowUp, ChevronDown, ChevronRight, Clock } from "lucide-react";
import { CopyButton } from "@/components/ui/copy-button";
import type { BranchPoint, Node } from "@/ipc/bindings";
import { stripAnsi } from "@/lib/ansi";
import { Markdown } from "@/lib/markdown";
import { useUi } from "@/state/ui";
import { BranchSwitcher } from "../BranchSwitcher";
import { describeNotice } from "../notice";
import { ImageRefs } from "../tools";
import { clock, prettyJson } from "../util";
import { Anchor } from "./Anchor";
import { scrollToNode, useTranscriptCtx } from "./scroll";

type UserBody = Extract<Node["body"], { kind: "userPrompt" }>;

/** Human / command prompt (turn start), with the branch switcher when it is a branch head. */
export function UserPromptView({ node, body, branch }: { node: Node; body: UserBody; branch?: BranchPoint | null }) {
  const o = body.origin;
  if (o.kind === "teammate") return <TeammateCard node={node} body={body} />;
  if (o.kind !== "human" && o.kind !== "command") {
    return (
      <div data-testid={o.kind === "commandOutput" ? "command-output" : undefined} className="flex items-baseline gap-2 text-[12px] text-secondary">
        <span className="shrink-0">{o.kind === "meta" ? "元消息" : o.kind === "taskNotification" ? "任务通知" : "命令输出"}</span>
        <span className="min-w-0 truncate">{o.kind === "commandOutput" ? outputText(body.text) : body.text}</span>
      </div>
    );
  }
  return (
    <Anchor nodeId={node.id} className="group/copy flex flex-col items-end gap-1.5">
      <div className="flex items-center gap-2 text-[11px] text-secondary">
        {branch && <BranchSwitcher point={branch} />}
        <CopyButton text={o.kind === "command" ? `${o.name} ${o.args}`.trim() : body.text} />
        <span>你 · {clock(node.timestampMs)}</span>
      </div>
      <div className="max-w-[640px] rounded-xl border border-selection bg-selection/50 px-3.5 py-2.5 break-words whitespace-pre-wrap">
        {o.kind === "command" ? (
          <>
            {/* Like the terminal: the command token stands out, its arguments read as prose. */}
            <code data-testid="command-name" className="font-mono text-[12.5px] font-semibold text-accent">
              {o.name}
            </code>
            {o.args && <> {o.args}</>}
          </>
        ) : (
          body.text
        )}
      </div>
      {body.images.length > 0 && <PromptImages node={node} body={body} />}
    </Anchor>
  );
}

/** A command output without its `<local-command-stdout>`-style tags and ANSI codes. */
const outputText = (text: string) => stripAnsi(text.replace(/<\/?(?:local-command|bash)-std(?:out|err)>/g, "")).trim();

const STATUS_TEXT: Record<string, string> = { completed: "已完成", failed: "失败", killed: "已终止", stopped: "已停止", error: "失败" };

/** Task-notification prompt: a compact row for a background task's end, linking back to where it was started. */
export function NotificationRow({ node, body }: { node: Node; body: UserBody }) {
  const { runs, toolNode, scope } = useTranscriptCtx();
  const o = body.origin;
  if (o.kind !== "taskNotification") return null;
  const run = [...runs.values()].find((r) => (o.toolUseId && r.toolUseId === o.toolUseId) || (o.taskId && r.agentId === o.taskId));
  const cardNode = o.toolUseId ? toolNode.get(o.toolUseId) : undefined;
  const failed = o.status === "failed" || o.status === "error";
  const n = describeNotice(o.summary, run ? (run.description ?? run.agentType ?? o.taskId) : null);
  return (
    <Anchor
      nodeId={node.id}
      className="flex flex-wrap items-center gap-2 rounded-lg border border-dashed border-border px-3 py-2 text-[12px] text-text/80"
    >
      <Clock size={14} strokeWidth={1.5} className="shrink-0 text-secondary" aria-hidden />
      <span data-testid="task-notification" className="min-w-[200px] flex-1">
        {n.kind}{" "}
        {n.name && (
          <>
            <strong data-testid="task-notification-name" title={n.name} className="inline-block max-w-full truncate align-bottom font-semibold">
              {n.name}
            </strong>{" "}
          </>
        )}
        <span className={failed ? "text-error" : undefined}>{STATUS_TEXT[o.status ?? ""] ?? o.status ?? "有更新"}</span>
        {n.detail && <span className="text-secondary"> · {n.detail}</span>}
        <span className="ml-1.5 text-secondary">{clock(node.timestampMs)}</span>
      </span>
      {cardNode && o.toolUseId && (
        <button
          type="button"
          data-testid="notification-to-card"
          onClick={() => scrollToNode({ scope, nodeId: cardNode, toolUseId: o.toolUseId })}
          className="flex items-center gap-0.5 font-semibold text-accent hover:underline"
        >
          <ArrowUp size={12} strokeWidth={2} aria-hidden />
          查看派出位置
        </button>
      )}
    </Anchor>
  );
}

type TeammateOrigin = Extract<UserBody["origin"], { kind: "teammate" }>;

/** The `<teammate-message>` blocks of a relayed message (the injected notice after them is dropped). */
function teammateMessages(text: string): { id: string | null; body: string }[] {
  const out = [...text.matchAll(/<teammate-message\b([^>]*)>([\s\S]*?)<\/teammate-message>/g)].map((m) => ({
    id: /\bteammate_id="([^"]*)"/.exec(m[1])?.[1] || null,
    body: m[2].trim(),
  }));
  return out.length ? out : [{ id: null, body: text.trim() }];
}

/** One-line preview of a message body: a JSON message's `summary` (else its `type`), else the first non-empty line. */
function previewOf(body: string): string {
  try {
    const v: unknown = JSON.parse(body);
    if (v && typeof v === "object") {
      const { summary, type } = v as { summary?: unknown; type?: unknown };
      if (typeof summary === "string" && summary) return summary;
      if (typeof type === "string") return type;
    }
  } catch {
    /* plain text */
  }
  return body.split("\n").find((l) => l.trim())?.trim() ?? "";
}

const isJson = (body: string) => /^[[{]/.test(body) && prettyJson(body) !== body;

/** Message relayed from another Claude session of an agent team: not the user's prompt, a collapsed card. */
function TeammateCard({ node, body }: { node: Node; body: UserBody }) {
  const o = body.origin as TeammateOrigin;
  const key = `teammate:${node.id}`;
  const open = useUi((s) => s.expanded[key] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const messages = teammateMessages(body.text);
  const preview = o.summary ?? previewOf(messages[0].body);
  return (
    <div data-testid="teammate-message" className="overflow-hidden rounded-lg border border-border">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => toggle(key)}
        className="flex w-full items-center gap-2 bg-list px-2.5 py-1.5 text-left text-[12px] text-text/80"
      >
        {open ? <ChevronDown size={12} strokeWidth={1.8} aria-hidden /> : <ChevronRight size={12} strokeWidth={1.8} aria-hidden />}
        {o.color && <span aria-hidden className="size-2 shrink-0 rounded-full" style={{ backgroundColor: o.color }} />}
        <span className="shrink-0 font-semibold">Teammate · {o.teammateId ?? "未知"}</span>
        {messages.length > 1 && <span className="shrink-0 text-secondary">{messages.length} 条</span>}
        <span data-testid="teammate-preview" className="min-w-0 flex-1 truncate text-secondary">
          {preview}
        </span>
        <span className="shrink-0 text-secondary">{clock(node.timestampMs)}</span>
      </button>
      {open && (
        <div data-testid="teammate-body" className="flex flex-col gap-2 border-t border-border px-3 py-2 text-[12px]">
          {messages.map((m, i) => (
            <div key={i} className="flex flex-col gap-1">
              {messages.length > 1 && <span className="text-[11px] text-secondary">{m.id ?? "未知"}</span>}
              {isJson(m.body) ? (
                <pre className="m-0 max-h-60 overflow-auto rounded bg-code px-2.5 py-1.5 font-mono text-[12px] leading-[1.55] break-words whitespace-pre-wrap">
                  {prettyJson(m.body)}
                </pre>
              ) : (
                <Markdown text={m.body} />
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function PromptImages({ node, body }: { node: Node; body: UserBody }) {
  const { sessionId, scope } = useTranscriptCtx();
  return <ImageRefs images={body.images} ctx={{ sessionId, scope, nodeId: node.id }} />;
}
