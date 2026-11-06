import { ArrowUp, ChevronDown, ChevronRight, Clock } from "lucide-react";
import { CopyButton } from "@/components/ui/copy-button";
import { arrived, isFresh, RewindButton } from "@/features/connect";
import type { BranchPoint, Node } from "@/ipc/bindings";
import { stripAnsi } from "@/lib/ansi";
import { cn } from "@/lib/cn";
import { Markdown } from "@/lib/markdown";
import { useUi } from "@/state/ui";
import { BranchSwitcher } from "../BranchSwitcher";
import { describeNotice } from "../notice";
import { ImageRefs } from "../tools";
import { type TeammateMessage, type TeammatePart, previewOf, teammateParts } from "../teammate";
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
          body.text
        )}
      </div>
      {body.images.length > 0 && <PromptImages node={node} body={body} />}
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

/**
 * Message relayed from another Claude session of an agent team (not the user's prompt), part by part: reports as a
 * collapsed card; the end of a teammate's round with its result as a card of its own; idle, failed, shut down and task
 * assigned as one centered line.
 */
function TeammateCard({ node, body }: { node: Node; body: UserBody }) {
  const parts = teammateParts(body.text);
  return (
    <div className="flex flex-col gap-2">
      {parts.map((p, i) => {
        const key = `teammate:${node.id}:${i}`;
        if (p.kind === "report") return <TeammateReport key={key} node={node} messages={p.messages} stateKey={key} />;
        if (p.kind === "done") return <TeammateDone key={key} node={node} part={p} stateKey={key} />;
        return <TeammateEvent key={key} node={node} part={p} />;
      })}
    </div>
  );
}

function TeammateReport({ node, messages, stateKey }: { node: Node; messages: TeammateMessage[]; stateKey: string }) {
  const open = useUi((s) => s.expanded[stateKey] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const first = messages[0];
  return (
    <div data-testid="teammate-message" className="flex flex-col gap-1.5">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => toggle(stateKey)}
        className="flex w-full items-center gap-2 text-left text-[12px] text-secondary hover:text-text"
      >
        {open ? (
          <ChevronDown size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
        ) : (
          <ChevronRight size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
        )}
        <Dot color={first.color} />
        <span className="shrink-0 font-medium text-text/80">Teammate · {first.id ?? "未知"}</span>
        {messages.length > 1 && <span className="shrink-0 text-secondary">{messages.length} 条</span>}
        <span data-testid="teammate-preview" className="min-w-0 flex-1 truncate text-secondary">
          {previewOf(first)}
        </span>
        <span className="shrink-0 text-secondary">{clock(node.timestampMs)}</span>
      </button>
      {open && (
        <div
          data-testid="teammate-body"
          className="ml-1.5 flex flex-col gap-2 border-l-2 border-border pl-4 text-[12px]"
        >
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

/** A result longer than this many lines or characters starts clamped. */
const DONE_LINES = 8;
const DONE_CHARS = 600;

/** The end of a teammate's round, with its result: Claude Code's "Teammate @x finished". */
function TeammateDone({
  node,
  part,
  stateKey,
}: {
  node: Node;
  part: Extract<TeammatePart, { kind: "done" }>;
  stateKey: string;
}) {
  const open = useUi((s) => s.expanded[stateKey] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const long = part.result.split("\n").length > DONE_LINES || part.result.length > DONE_CHARS;
  return (
    <div data-testid="teammate-done" className="flex flex-col gap-1.5 rounded-[10px] border border-border px-3 py-2.5">
      <div className="flex items-center gap-2 text-[12px] text-secondary">
        <Dot color={part.color} />
        <span className="font-semibold text-text">{part.who}</span>
        <span className={cn("font-medium", part.failed ? "text-error" : "text-live")}>
          {part.failed ? "失败" : "完成"}
        </span>
        <span className="ml-auto">{clock(node.timestampMs)}</span>
      </div>
      <div
        data-testid="teammate-result"
        className={cn(
          long && !open && "max-h-[12em] overflow-hidden [mask-image:linear-gradient(to_bottom,black_65%,transparent)]",
        )}
      >
        <Markdown text={part.result} />
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
    </div>
  );
}

const EVENT_LABEL = { idle: "空闲", failed: "失败", exited: "已退出", task: "分配任务" } as const;

/** Idle, failed without a result, shut down, task assigned: one centered line. */
function TeammateEvent({ node, part }: { node: Node; part: Exclude<TeammatePart, { kind: "report" | "done" }> }) {
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

function PromptImages({ node, body }: { node: Node; body: UserBody }) {
  const { sessionId, scope } = useTranscriptCtx();
  return <ImageRefs images={body.images} ctx={{ sessionId, scope, nodeId: node.id }} />;
}
