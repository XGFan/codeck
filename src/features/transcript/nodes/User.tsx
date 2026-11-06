import { ArrowUp, ChevronDown, ChevronRight, Clock } from "lucide-react";
import type { ReactNode } from "react";
import { CopyButton } from "@/components/ui/copy-button";
import { arrived, isFresh, RewindButton } from "@/features/connect";
import type { BranchPoint, Node } from "@/ipc/bindings";
import { stripAnsi } from "@/lib/ansi";
import { cn } from "@/lib/cn";
import { Markdown } from "@/lib/markdown";
import { HighlightedCode } from "@/lib/shiki";
import { useUi } from "@/state/ui";
import { BranchSwitcher } from "../BranchSwitcher";
import { describeNotice } from "../notice";
import { promptParts } from "../promptCode";
import { ImageMarker, ImageRefs } from "../tools";
import { type TeammateItem, peerMessages, previewOf, teammateItems, teammateParts } from "../teammate";
import { clock, commandLine, prettyJson } from "../util";
import { TaskPromptCard } from "./Agents";
import { Anchor } from "./Anchor";
import { scrollToNode, useTranscriptCtx } from "./scroll";

type UserBody = Extract<Node["body"], { kind: "userPrompt" }>;

/** Human / command prompt (turn start), with the branch switcher when it is a branch head. */
export function UserPromptView({ node, body, branch }: { node: Node; body: UserBody; branch?: BranchPoint | null }) {
  const { rewindable } = useTranscriptCtx();
  const o = body.origin;
  if (o.kind === "teammate") return <TeammateCard node={node} body={body} />;
  if (o.kind === "peer") return <PeerCard node={node} body={body} />;
  if (o.kind === "coordinator") return <CoordinatorCard node={node} body={body} />;
  if (o.kind !== "human" && o.kind !== "command") {
    return (
      <div
        data-testid={o.kind === "commandOutput" ? "command-output" : undefined}
        className="flex items-baseline gap-2 text-[12px] text-secondary"
      >
        <span className="shrink-0">
          {o.kind === "meta" ? "元消息" : o.kind === "taskNotification" ? "任务通知" : "命令输出"}
        </span>
        <span className="min-w-0 truncate">{o.kind === "commandOutput" ? outputText(body.text) : body.text}</span>
      </div>
    );
  }
  // Just sent from the input box: it settles in where its placeholder was (once).
  const fresh = isFresh(node.id);
  return (
    <Anchor nodeId={node.id} className="group/copy flex flex-col items-end gap-1">
      <div
        data-testid="user-bubble"
        data-arrived={fresh || undefined}
        onAnimationEnd={fresh ? () => arrived(node.id) : undefined}
        className={cn(
          "max-w-[560px] rounded-2xl bg-selection/60 px-3.5 py-2 text-[14px] leading-[1.55] break-words whitespace-pre-wrap",
          fresh && "prompt-arrive",
        )}
      >
        {o.kind === "command" ? (
          <>
            {/* Like the terminal: the command token stands out, its arguments read as prose. */}
            <code data-testid="command-name" className="font-mono text-[12.5px] font-semibold text-accent">
              {o.name}
            </code>
            {/* `!` shell commands read as typed: `!git status`. */}
            {o.args && (o.name === "!" ? o.args : <> {o.args}</>)}
          </>
        ) : (
          <PromptText text={body.text} node={node} body={body} />
        )}
      </div>
      {/* The images the text does not place (`[Image #n]`), below it; a command's, all of them. */}
      {body.images.length > (o.kind === "command" ? 0 : markerCount(body.text)) && (
        <PromptImages node={node} body={body} skip={o.kind === "command" ? 0 : markerCount(body.text)} />
      )}
      {/* Under the bubble, as in Codex: time, rewind and copy on hover, the branch switcher at a branch head. */}
      <div
        data-testid="prompt-meta"
        className={cn("flex items-center gap-1.5 text-[11.5px] text-secondary", fresh && "prompt-arrive-meta")}
      >
        {rewindable.has(node.id) && <RewindButton nodeId={node.id} />}
        <CopyButton text={o.kind === "command" ? commandLine(o.name, o.args) : body.text} />
        {branch && <BranchSwitcher point={branch} />}
        <span>{clock(node.timestampMs)}</span>
      </div>
    </Anchor>
  );
}

const MARKER = /\[Image #(\d+)\]/g;

/** `[Image #n]` placeholders in a prompt's words (not in its code), one per image in order. */
const markerCount = (text: string) =>
  promptParts(text).reduce((n, p) => n + (p.kind === "text" ? (p.text.match(MARKER)?.length ?? 0) : 0), 0);

/**
 * A typed prompt as typed, with its `inline code` and ```fenced blocks``` set as code (nothing else is markdown),
 * and each `[Image #n]` as its image (the k-th placeholder is the k-th image).
 */
function PromptText({ text, node, body }: { text: string; node: Node; body: UserBody }) {
  const { sessionId, scope } = useTranscriptCtx();
  const ctx = { sessionId, scope, nodeId: node.id };
  let k = 0;
  const words = (s: string, key: number) => {
    const out: ReactNode[] = [];
    let at = 0;
    for (const m of s.matchAll(MARKER)) {
      if (m.index > at) out.push(s.slice(at, m.index));
      out.push(<ImageMarker key={`${key}:${m.index}`} image={body.images[k++]} label={`图片 #${m[1]}`} ctx={ctx} />);
      at = m.index + m[0].length;
    }
    out.push(s.slice(at));
    return out;
  };
  return promptParts(text).map((p, i) =>
    p.kind === "text" ? (
      words(p.text, i)
    ) : p.kind === "code" ? (
      <code key={i} data-testid="prompt-code" className="rounded bg-code px-[5px] py-px font-mono text-[12.5px]">
        {p.text}
      </code>
    ) : (
      <pre
        key={i}
        data-testid="prompt-code-block"
        className="my-1.5 overflow-x-auto rounded-md bg-code px-3 py-2 font-mono text-[12px] leading-[1.55] whitespace-pre"
      >
        <code>
          <HighlightedCode code={p.text} lang={p.lang} />
        </code>
      </pre>
    ),
  );
}

/** In a Subagent Run: a message its parent sent it (SendMessage), shown like the task prompt. */
function CoordinatorCard({ node, body }: { node: Node; body: UserBody }) {
  const { scope, runs } = useTranscriptCtx();
  const parent = scope.kind === "subagent" ? runs.get(scope.agentId)?.parentAgentId : null;
  return (
    <TaskPromptCard node={node} text={body.text} title={parent ? "上级 Subagent 发来的消息" : "主对话发来的消息"} />
  );
}

/** A command output without its `<local-command-stdout>`-style tags and ANSI codes. */
const outputText = (text: string) => stripAnsi(text.replace(/<\/?(?:local-command|bash)-std(?:out|err)>/g, "")).trim();

const STATUS_TEXT: Record<string, string> = {
  completed: "已完成",
  failed: "失败",
  killed: "已终止",
  stopped: "已停止",
  error: "失败",
};

/** Task-notification prompt: a compact row for a background task's end, linking back to where it was started. */
export function NotificationRow({ node, body }: { node: Node; body: UserBody }) {
  const { runs, toolNode, scope } = useTranscriptCtx();
  const o = body.origin;
  if (o.kind !== "taskNotification") return null;
  const run = [...runs.values()].find(
    (r) => (o.toolUseId && r.toolUseId === o.toolUseId) || (o.taskId && r.agentId === o.taskId),
  );
  // The launch is the run's Agent call; after a SendMessage resumed the agent, the notification names that call instead.
  const launch = run?.toolUseId ?? o.toolUseId;
  const cardNode = launch ? toolNode.get(launch) : undefined;
  const resumedBy = o.toolUseId && o.toolUseId !== launch ? o.toolUseId : null;
  const resumeNode = resumedBy ? toolNode.get(resumedBy) : undefined;
  const failed = o.status === "failed" || o.status === "error";
  const n = describeNotice(o.summary, run ? (run.description ?? run.agentType ?? o.taskId) : null);
  return (
    <Anchor
      nodeId={node.id}
      className="flex flex-wrap items-center justify-center gap-x-2 gap-y-0.5 px-3 text-[12px] text-secondary"
    >
      <Clock size={13} strokeWidth={1.5} className="shrink-0" aria-hidden />
      <span data-testid="task-notification" className="min-w-0 max-w-full text-center">
        {n.kind}{" "}
        {n.name && (
          <>
            <strong
              data-testid="task-notification-name"
              title={n.name}
              className="inline-block max-w-[40ch] truncate align-bottom font-medium text-text/80"
            >
              {n.name}
            </strong>{" "}
          </>
        )}
        <span className={failed ? "text-error" : undefined}>{STATUS_TEXT[o.status ?? ""] ?? o.status ?? "有更新"}</span>
        {n.detail && <span className="text-secondary"> · {n.detail}</span>}
        <span className="ml-1.5 text-secondary">{clock(node.timestampMs)}</span>
      </span>
      {cardNode && launch && (
        <button
          type="button"
          data-testid="notification-to-card"
          onClick={() => scrollToNode({ scope, nodeId: cardNode, toolUseId: launch })}
          className="flex items-center gap-0.5 text-accent hover:underline"
        >
          <ArrowUp size={12} strokeWidth={2} aria-hidden />
          查看派出位置
        </button>
      )}
      {resumeNode && resumedBy && (
        <button
          type="button"
          data-testid="notification-to-resume"
          onClick={() => scrollToNode({ scope, nodeId: resumeNode, toolUseId: resumedBy })}
          className="flex items-center gap-0.5 text-accent hover:underline"
        >
          <ArrowUp size={12} strokeWidth={2} aria-hidden />
          查看续接位置
        </button>
      )}
    </Anchor>
  );
}

const isJson = (body: string) => /^[[{]/.test(body) && prettyJson(body) !== body;

const Dot = ({ color }: { color: string | null }) =>
  color ? <span aria-hidden className="size-2 shrink-0 rounded-full" style={{ backgroundColor: color }} /> : null;

/** A body longer than this many lines or characters starts clamped. */
const CLAMP_LINES = 8;
const CLAMP_CHARS = 600;

/** A message body: Markdown at the size of other prose (a JSON message pretty-printed), clamped when long. */
function MessageBody({ text, stateKey, testId }: { text: string; stateKey: string; testId?: string }) {
  const open = useUi((s) => s.expanded[stateKey] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const long = text.split("\n").length > CLAMP_LINES || text.length > CLAMP_CHARS;
  return (
    <>
      <div
        data-testid={testId}
        className={cn(
          long && !open && "max-h-[12em] overflow-hidden [mask-image:linear-gradient(to_bottom,black_65%,transparent)]",
        )}
      >
        {isJson(text) ? (
          <pre className="m-0 overflow-auto rounded bg-code px-2.5 py-1.5 font-mono text-[12px] leading-[1.55] break-words whitespace-pre-wrap">
            {prettyJson(text)}
          </pre>
        ) : (
          <Markdown text={text} className="text-[14px] leading-[1.7]" />
        )}
      </div>
      {long && (
        <button
          type="button"
          aria-expanded={open}
          onClick={() => toggle(stateKey)}
          className="self-start text-[12px] text-accent hover:underline"
        >
          {open ? "收起" : "展开全文"}
        </button>
      )}
    </>
  );
}

/**
 * A message from another agent as a card: its color, who, what kind (报告 / 完成 / 失败 / 会话 …) and when, then
 * the body. Teammate reports and results, and messages from other Sessions, agents and plugins share it.
 */
function MessageCard({
  testId,
  color,
  who,
  tag,
  tone,
  timeMs,
  title,
  children,
}: {
  testId: string;
  color: string | null;
  who: string;
  tag: string;
  tone?: "live" | "error";
  timeMs: number | null;
  title?: string | null;
  children: ReactNode;
}) {
  return (
    <div data-testid={testId} className="flex flex-col gap-1.5 rounded-[10px] border border-border px-3 py-2.5">
      <div className="flex items-center gap-2 text-[12px] text-secondary">
        <Dot color={color} />
        <span className="font-semibold text-text">{who}</span>
        <span
          data-testid="message-tag"
          className={cn(
            "rounded border px-1 text-[11px] leading-[16px]",
            tone === "live"
              ? "border-live/35 text-live"
              : tone === "error"
                ? "border-error/35 text-error"
                : "border-border",
          )}
        >
          {tag}
        </span>
        <span className="ml-auto shrink-0">{clock(timeMs)}</span>
      </div>
      {title && (
        <div data-testid="message-title" className="text-[14px] font-semibold">
          {title}
        </div>
      )}
      {children}
    </div>
  );
}

/**
 * Message relayed from another Claude session of an agent team (not the user's prompt), item by item: each report a
 * card; the end of a teammate's round with its result a card, holding the report it sent just before; idle, failed,
 * shut down and task assigned as one centered line.
 */
function TeammateCard({ node, body }: { node: Node; body: UserBody }) {
  const items = teammateItems(teammateParts(body.text));
  return (
    <div className="flex flex-col gap-2">
      {items.map((it, i) => {
        const key = `teammate:${node.id}:${i}`;
        if (it.kind === "report")
          return (
            <MessageCard
              key={key}
              testId="teammate-message"
              color={it.message.color}
              who={it.message.id ?? "未知"}
              tag="报告"
              timeMs={node.timestampMs}
              title={it.message.summary}
            >
              <MessageBody text={it.message.body} stateKey={key} testId="teammate-body" />
            </MessageCard>
          );
        if (it.kind === "done") return <TeammateDone key={key} node={node} item={it} stateKey={key} />;
        return <TeammateEvent key={key} node={node} part={it} />;
      })}
    </div>
  );
}

const PEER_LABEL = { session: "会话", agent: "Agent", plugin: "插件" } as const;

/** A message another agent sent with SendMessage (another Session, a Subagent or Teammate), or a plugin's. */
function PeerCard({ node, body }: { node: Node; body: UserBody }) {
  const o = body.origin as Extract<UserBody["origin"], { kind: "peer" }>;
  return (
    <div className="flex flex-col gap-2">
      {peerMessages(body.text, o.from).map((m, i) => (
        <MessageCard
          key={i}
          testId="peer-message"
          color={m.color}
          who={m.id ?? "未知"}
          tag={PEER_LABEL[o.source]}
          timeMs={node.timestampMs}
          title={m.summary}
        >
          <MessageBody text={m.body} stateKey={`peer:${node.id}:${i}`} testId="teammate-body" />
        </MessageCard>
      ))}
    </div>
  );
}

/** The end of a teammate's round, with its result (Claude Code's "Teammate @x finished") and the report before it. */
function TeammateDone({
  node,
  item,
  stateKey,
}: {
  node: Node;
  item: Extract<TeammateItem, { kind: "done" }>;
  stateKey: string;
}) {
  const reportKey = `${stateKey}:report`;
  const open = useUi((s) => s.expanded[reportKey] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const r = item.report;
  return (
    <MessageCard
      testId="teammate-done"
      color={item.color}
      who={item.who}
      tag={item.failed ? "失败" : "完成"}
      tone={item.failed ? "error" : "live"}
      timeMs={node.timestampMs}
    >
      <MessageBody text={item.result} stateKey={stateKey} testId="teammate-result" />
      {r && (
        <div data-testid="teammate-done-report" className="mt-0.5 flex flex-col gap-1.5 border-t border-border pt-1.5">
          <button
            type="button"
            aria-expanded={open}
            onClick={() => toggle(reportKey)}
            className="flex min-w-0 items-center gap-1.5 text-left text-[12px] text-secondary hover:text-text"
          >
            {open ? (
              <ChevronDown size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
            ) : (
              <ChevronRight size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
            )}
            <span className="shrink-0">报告</span>
            <span className="min-w-0 truncate">{previewOf(r)}</span>
          </button>
          {open && (
            <div className="pl-[18px]">
              <MessageBody text={r.body} stateKey={`${reportKey}:body`} />
            </div>
          )}
        </div>
      )}
    </MessageCard>
  );
}

const EVENT_LABEL = { idle: "空闲", failed: "失败", exited: "已退出", task: "分配任务" } as const;

/** Idle, failed without a result, shut down, task assigned: one centered line. */
function TeammateEvent({ node, part }: { node: Node; part: Exclude<TeammateItem, { kind: "report" | "done" }> }) {
  const detail =
    part.kind === "failed"
      ? part.reason
      : part.kind === "task"
        ? [part.taskId && `#${part.taskId}`, part.subject].filter(Boolean).join(" ") || null
        : null;
  return (
    <div
      data-testid="teammate-event"
      data-kind={part.kind}
      className="flex items-center gap-2 text-[12px] text-secondary"
    >
      <span className="h-px flex-1 bg-border" />
      <Dot color={part.color} />
      <span className="min-w-0 max-w-[80%] truncate">
        <strong className="font-medium text-text/80">{part.who}</strong>{" "}
        <span className={part.kind === "failed" ? "text-error" : undefined}>{EVENT_LABEL[part.kind]}</span>
        {detail && ` · ${detail}`} · {clock(node.timestampMs)}
      </span>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}

function PromptImages({ node, body, skip }: { node: Node; body: UserBody; skip: number }) {
  const { sessionId, scope } = useTranscriptCtx();
  return <ImageRefs images={body.images.slice(skip)} ctx={{ sessionId, scope, nodeId: node.id }} />;
}
