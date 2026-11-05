import { MessageCircleQuestionMark, ScanSearch, Sparkles } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { LiveDot } from "@/components/LiveDot";
import { Button } from "@/components/ui/button";
import { scrollToNode } from "@/features/transcript/nodes/scroll";
import { toolKey } from "@/features/transcript/keys";
import { PendingText, ToolCallView } from "@/features/transcript/tools";
import type { ToolCall } from "@/ipc/bindings";
import { useCurrentTranscript } from "@/queries";
import { useUi } from "@/state/ui";
import type { ConnState, Request } from "./conn";
import type { ChoiceEvent, LayoutSource, PermissionRequestEvent, QuestionEvent } from "./protocol";
import { NoticeCard } from "./Notice";
import { QuestionForm } from "./QuestionForm";
import { useConnect } from "./store";
import { useAnswer, useNumberKeys } from "./useAnswer";

/** Cards for what the Session waits on, main line first. */
export function Requests({ c }: { c: ConnState }) {
  const reqs = [...c.requests].sort((a, b) => Number(isSub(a)) - Number(isSub(b)));
  if (reqs.length === 0) {
    // Waiting with nothing to show yet: the analyzer may be looking at the screen.
    if (c.state === "waiting" && c.waiting?.kind !== "question")
      return (
        <NoticeCard
          testId="analyzing"
          tone="wait"
          icon={<ScanSearch size={18} strokeWidth={1.7} />}
          title="正在识别界面…"
          desc="Claude Code 弹出了认不出的对话框，分析器在看屏幕。"
        />
      );
    return null;
  }
  return (
    <div
      className="flex max-h-[45vh] flex-col gap-2 overflow-y-auto"
      data-testid="connect-requests"
      data-connect-requests
    >
      {reqs.map((r, i) => (
        <Card key={r.request_id} r={r} c={c} focus={i === 0} />
      ))}
    </div>
  );
}

const isSub = (r: Request) => "agent_id" in r && !!r.agent_id;

function Card({ r, c, focus }: { r: Request; c: ConnState; focus: boolean }) {
  switch (r.type) {
    case "permission_request":
      return <PermissionCard r={r} c={c} focus={focus} />;
    case "question":
      return r.agent_id ? <QuestionCard r={r} c={c} /> : <QuestionHint r={r} />;
    case "choice":
      return <ChoiceCard r={r} c={c} focus={focus} />;
    default:
      return <ScreenCard r={r} c={c} />;
  }
}

/**
 * A screen ccrpc cannot read (a panel such as `/cost`'s, an unknown dialog): shown as is; Esc closes it
 * (`abort` while busy / waiting; otherwise ccrpc refuses it, so the key itself).
 */
function ScreenCard({ r, c }: { r: Extract<Request, { type: "dialog" | "unrecognized" }>; c: ConnState }) {
  const command = useConnect((s) => s.command);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = async () => {
    setBusy(true);
    setError(null);
    const esc = c.state === "busy" || c.state === "waiting" ? { type: "abort" } : { type: "send_keys", keys: ["esc"] };
    const r = await command(c.conn, esc);
    setBusy(false);
    if (!r.success) setError(`关闭失败：${r.error?.message ?? r.error?.code ?? ""}`);
  };
  return (
    <Shell
      title={r.type === "dialog" ? "Claude Code 停在一个 ccrpc 认不出的界面" : `认不出的屏幕（${r.context}）`}
      testid="screen-card"
    >
      <pre className="max-h-64 overflow-auto rounded bg-terminal p-2 font-mono text-[11px] leading-[1.45] text-[#e4e4e6]">
        {trimLines(r.lines).join("\n")}
      </pre>
      <div className="mt-2 flex items-center gap-2">
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void close()} data-testid="screen-close">
          关闭（Esc）
        </Button>
        <span className="text-[12px] text-secondary">其他操作需在 Mac Client 的终端里处理。</span>
        {error && <span className="text-[12px] text-error">{error}</span>}
      </div>
    </Shell>
  );
}

function trimLines(lines: string[]): string[] {
  let a = 0;
  let b = lines.length;
  while (a < b && !lines[a]!.trim()) a++;
  while (b > a && !lines[b - 1]!.trim()) b--;
  return lines.slice(a, b);
}

function Shell({
  title,
  source,
  agent,
  testid,
  children,
}: {
  title: string;
  source?: LayoutSource | null;
  agent?: string | null;
  testid: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-lg border border-live-waiting/60 bg-ground px-3 py-2.5 shadow-sm" data-testid={testid}>
      <div className="mb-2 flex items-center gap-2">
        <LiveDot state="waiting" />
        <h3 className="m-0 min-w-0 flex-1 truncate text-[13px] font-semibold">{title}</h3>
        {agent && (
          <span className="rounded bg-code px-1.5 text-[11px] text-secondary">Subagent {agent.slice(0, 8)}</span>
        )}
        {source === "learned" && (
          <span
            className="flex items-center gap-1 rounded bg-selection px-1.5 text-[11px] text-accent"
            title="分析器学到的布局"
            data-testid="ai-badge"
          >
            <Sparkles size={11} aria-hidden /> AI 识别
          </span>
        )}
      </div>
      {children}
    </section>
  );
}

/** The tool's input, drawn by the transcript's tool renderer (open, with no result yet). */
function ToolInput({ r, sessionId }: { r: PermissionRequestEvent; sessionId: string | null }) {
  const call = useMemo<ToolCall>(
    () => ({
      toolUseId: r.request_id,
      name: r.tool_name,
      inputJson: JSON.stringify(r.input ?? {}),
      inputTruncated: false,
      result: null,
      subagentId: null,
      workflowRunId: null,
      notificationNodeId: null,
    }),
    [r.request_id, r.tool_name, r.input],
  );
  const nodeId = `connect:${r.request_id}`;
  const key = toolKey(nodeId, r.request_id);
  const known = useUi((s) => key in s.expanded);
  useEffect(() => {
    if (!known) useUi.getState().setExpanded(key, true);
  }, [key, known]);
  return (
    <PendingText.Provider value="等待审批">
      <ToolCallView nodeId={nodeId} call={call} startMs={null} sessionId={sessionId ?? undefined} />
    </PendingText.Provider>
  );
}

function PermissionCard({ r, c, focus }: { r: PermissionRequestEvent; c: ConnState; focus: boolean }) {
  const { busy, error, send } = useAnswer(c);
  const [feedback, setFeedback] = useState<string | null>(null);
  const always = r.options?.find((o) => o.meaning === "allow_always");
  // A learned layout's meanings come from a model: its buttons show the dialog's own words, so a
  // wrongly labelled option is visible before it is pressed.
  const label = (meaning: string, fallback: string) =>
    r.layout_source === "learned" ? (r.options?.find((o) => o.meaning === meaning)?.label ?? fallback) : fallback;
  const answer = (decision: string, message?: string) =>
    send({ type: "answer_permission", request_id: r.request_id, decision, ...(message ? { message } : {}) });
  const actions = [
    () => void answer("allow"),
    ...(always ? [() => void answer("allow_always")] : []),
    () => void answer("deny"),
  ];
  useNumberKeys(focus && feedback == null && !busy, actions);
  let n = 0;
  return (
    <Shell title={`允许 ${r.tool_name}？`} source={r.layout_source} agent={r.agent_id} testid="permission-card">
      <ToolInput r={r} sessionId={c.sessionId} />
      <div className="mt-2.5 flex flex-wrap items-center gap-1.5">
        <Button size="sm" disabled={busy} onClick={() => void answer("allow")} data-testid="allow" data-card-answer>
          {++n}. {label("allow", "允许")}
        </Button>
        {always && (
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => void answer("allow_always")}
            title={always.label}
            data-testid="allow-always"
            className="max-w-[28rem]"
          >
            <span className="truncate">
              {++n}. {always.label}
            </span>
          </Button>
        )}
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void answer("deny")} data-testid="deny">
          {++n}. {label("deny", "拒绝")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={busy}
          onClick={() => setFeedback(feedback == null ? "" : null)}
          data-testid="deny-with-message"
        >
          拒绝，并告诉 Claude…
        </Button>
      </div>
      {feedback != null && (
        <form
          className="mt-2 flex gap-1.5"
          onSubmit={(e) => {
            e.preventDefault();
            if (feedback.trim()) void answer("deny", feedback.trim());
          }}
        >
          <input
            autoFocus
            value={feedback}
            onChange={(e) => setFeedback(e.target.value)}
            placeholder="要 Claude 怎么改（本轮继续）"
            className="h-7 min-w-0 flex-1 rounded-md border border-border bg-ground px-2 text-[13px] outline-none focus:border-accent"
            data-testid="deny-message"
          />
          <Button size="sm" type="submit" disabled={busy || !feedback.trim()}>
            发送
          </Button>
        </form>
      )}
      {error && <p className="mt-1.5 text-[12px] text-error">{error}</p>}
    </Shell>
  );
}

/**
 * A main-line AskUserQuestion is answered on its 「向你提问」 card in the transcript ([`QuestionForm`]); the
 * input area only says so, with a jump to it (the card may still be on its way while the transcript updates).
 */
function QuestionHint({ r }: { r: QuestionEvent }) {
  const { data } = useCurrentTranscript();
  const node =
    data?.scope.kind === "main"
      ? data.nodes.find(
          (n) =>
            n.body.kind === "assistant" &&
            n.body.blocks.some((b) => b.kind === "toolCall" && b.toolUseId === r.request_id),
        )
      : undefined;
  const qs = Array.isArray(r.questions) ? r.questions : [];
  return (
    <NoticeCard
      testId="question-hint"
      tone="wait"
      icon={<MessageCircleQuestionMark size={18} strokeWidth={1.7} />}
      title="Claude 在问你"
      desc={
        qs.length
          ? `${qs[0]!.question}${qs.length > 1 ? ` 等 ${qs.length} 个问题` : ""} · 在对话里那张卡片上作答`
          : "在对话里那张卡片上作答"
      }
      action={
        node
          ? {
              label: "跳到提问",
              primary: true,
              testId: "question-jump",
              onClick: () =>
                scrollToNode({ scope: { kind: "main" }, nodeId: node.id, toolUseId: r.request_id, flashMs: 2000 }),
            }
          : undefined
      }
    />
  );
}

/** A background Subagent's AskUserQuestion: not in the main transcript, so a card of its own here. */
function QuestionCard({ r, c }: { r: QuestionEvent; c: ConnState }) {
  const qs = Array.isArray(r.questions) ? r.questions : [];
  return (
    <section
      data-testid="question-card"
      className="w-full overflow-hidden rounded-lg border border-live-waiting/60 shadow-sm"
    >
      <div className="flex items-center gap-2 border-b border-border bg-list px-2.5 py-1.5 text-[12px] text-text/80">
        <MessageCircleQuestionMark size={14} strokeWidth={1.6} className="text-accent" aria-hidden />
        <span className="font-semibold">向你提问</span>
        {qs.length > 1 && <span className="text-secondary">{qs.length} 个问题</span>}
        {r.agent_id && (
          <span className="rounded bg-code px-1.5 text-[11px] text-secondary">Subagent {r.agent_id.slice(0, 8)}</span>
        )}
        <span className="ml-auto text-live-waiting">等待回答</span>
      </div>
      <div className="bg-ground px-3 py-2.5">
        <QuestionForm r={r} c={c} />
      </div>
    </section>
  );
}

function ChoiceCard({ r, c, focus }: { r: ChoiceEvent; c: ConnState; focus: boolean }) {
  const { busy, error, send } = useAnswer(c);
  const pick = (label: string) => void send({ type: "answer_choice", request_id: r.request_id, option: label });
  useNumberKeys(
    focus && !busy,
    r.options.map((o) => () => pick(o.label)),
  );
  return (
    <Shell title={r.title} source={r.layout_source} testid="choice-card">
      <div className="flex flex-col items-start gap-1.5">
        {r.options.map((o, i) => (
          <Button
            key={o.label}
            size="sm"
            variant={i === 0 ? "default" : "outline"}
            disabled={busy}
            onClick={() => pick(o.label)}
            title={o.description ?? undefined}
            data-testid="choice-option"
            data-card-answer
            className="max-w-full"
          >
            <span className="truncate">
              {i + 1}. {o.label}
            </span>
          </Button>
        ))}
      </div>
      {error && <p className="mt-1.5 text-[12px] text-error">{error}</p>}
    </Shell>
  );
}
