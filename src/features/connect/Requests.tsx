import {
  ListChecks,
  MessageCircleQuestionMark,
  MonitorDot,
  RotateCcw,
  ScanSearch,
  ShieldQuestionMark,
  Sparkles,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import { scrollToNode } from "@/features/transcript/nodes/scroll";
import { toolKey } from "@/features/transcript/keys";
import { PendingText, ToolCallView } from "@/features/transcript/tools";
import type { ToolCall } from "@/ipc/bindings";
import { useCurrentTranscript } from "@/queries";
import { useUi } from "@/state/ui";
import { permissionLabel } from "./answers";
import type { ConnState, Request } from "./conn";
import type {
  ChoiceEvent,
  LayoutSource,
  PermissionRequestEvent,
  QuestionEvent,
  RewindOption,
  RewindRequest,
} from "./protocol";
import { NoticeCard } from "./Notice";
import { QuestionForm } from "./QuestionForm";
import { codeLine, REWIND_DESC, REWIND_LABELS, REWIND_ORDER, REWIND_WARNING } from "./rewind";
import { useConnect } from "./store";
import { useAnswer, useNumberKeys } from "./useAnswer";

/** Cards for what the Session waits on, main line first. */
export function Requests({ c }: { c: ConnState }) {
  const reqs = [...c.requests].sort((a, b) => Number(isSub(a)) - Number(isSub(b)));
  if (reqs.length === 0) {
    // ccrpc is finding the prompt in the terminal's Rewind picker (not a dialog for the analyzer).
    if (c.rewinding)
      return (
        <NoticeCard
          testId="rewinding"
          tone="wait"
          icon={<RotateCcw size={18} strokeWidth={1.7} />}
          title="正在准备回退…"
        />
      );
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
    case "rewind":
      return <RewindCard r={r} c={c} focus={focus} />;
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
      icon={<MonitorDot size={14} strokeWidth={1.6} />}
      title={r.type === "dialog" ? "Claude Code 停在一个 ccrpc 认不出的界面" : `认不出的屏幕（${r.context}）`}
      status="等待处理"
      testid="screen-card"
    >
      <pre className="max-h-64 overflow-auto rounded bg-terminal p-2 font-mono text-[11px] leading-[1.45] text-[#e4e4e6]">
        {trimLines(r.lines).join("\n")}
      </pre>
      <div className="flex items-center gap-2">
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

/**
 * A request card, as the transcript's 「向你提问」 card: a header bar (icon, title, the Subagent / AI badges, what it
 * waits for at the right), then the body.
 */
function Shell({
  icon,
  title,
  status,
  source,
  agent,
  testid,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  status: string;
  source?: LayoutSource | null;
  agent?: string | null;
  testid: string;
  children: React.ReactNode;
}) {
  return (
    <section
      className="w-full shrink-0 overflow-hidden rounded-lg border border-live-waiting/60 shadow-sm"
      data-testid={testid}
    >
      <div className="flex items-center gap-2 border-b border-border bg-list px-2.5 py-1.5 text-[12px] text-text/80">
        <span className="flex shrink-0 text-accent" aria-hidden>
          {icon}
        </span>
        <h3 className="m-0 min-w-0 truncate text-[12px] font-semibold">{title}</h3>
        {agent && (
          <span className="shrink-0 rounded bg-code px-1.5 text-[11px] text-secondary">
            Subagent {agent.slice(0, 8)}
          </span>
        )}
        {source === "learned" && (
          <span
            className="flex shrink-0 items-center gap-1 rounded bg-selection px-1.5 text-[11px] text-accent"
            title="分析器学到的布局"
            data-testid="ai-badge"
          >
            <Sparkles size={11} aria-hidden /> AI 识别
          </span>
        )}
        <span className="ml-auto shrink-0 text-live-waiting">{status}</span>
      </div>
      <div className="flex flex-col gap-2 bg-ground px-3 py-2.5 text-[12.5px]">{children}</div>
    </section>
  );
}

/**
 * One answer of a card: a full-width row with its number key. `primary` (the dialog's default) is drawn picked;
 * keyboard focus draws a row the same.
 */
function OptionRow({
  n,
  label,
  desc,
  primary,
  disabled,
  onClick,
  testid,
}: {
  n: number;
  label: string;
  desc?: string | null;
  primary?: boolean;
  disabled?: boolean;
  onClick: () => void;
  testid: string;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      data-testid={testid}
      data-card-answer
      className={cn(
        "group flex w-full items-center gap-2 rounded-md border px-2.5 py-1.5 text-left text-[12px] outline-none focus-visible:border-accent/50 focus-visible:bg-selection disabled:opacity-60",
        primary ? "border-accent/50 bg-selection" : "border-border hover:bg-selection/60",
      )}
    >
      <span
        aria-hidden
        className={cn(
          "inline-flex h-[18px] min-w-[18px] shrink-0 items-center justify-center rounded border text-[11px] tabular-nums",
          primary
            ? "border-accent bg-accent text-white"
            : "border-border bg-list text-secondary group-focus-visible:border-accent group-focus-visible:bg-accent group-focus-visible:text-white",
        )}
      >
        {n}
      </span>
      <span className="min-w-0 break-words">
        <span className="font-medium">{label}</span>
        {desc && <span className="ml-2 text-secondary">{desc}</span>}
      </span>
    </button>
  );
}

/** Under a card's answers: the number keys hint, an error, and `children` at the right. */
function CardFoot({ error, children }: { error?: string | null; children?: React.ReactNode }) {
  return (
    <div className="flex min-h-6 items-center gap-2 text-[11.5px] text-secondary">
      <span className="shrink-0">按数字键作答</span>
      {error && <span className="min-w-0 text-[12px] text-error">{error}</span>}
      {children && <span className="ml-auto flex shrink-0 items-center">{children}</span>}
    </div>
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
  const [feedback, setFeedback] = useState("");
  const always = r.options?.find((o) => o.meaning === "allow_always");
  const label = (meaning: string, fallback: string) => permissionLabel(r, meaning, fallback);
  const answer = (decision: string, message?: string) =>
    send({ type: "answer_permission", request_id: r.request_id, decision, ...(message ? { message } : {}) });
  const actions = [
    () => void answer("allow"),
    ...(always ? [() => void answer("allow_always")] : []),
    () => void answer("deny"),
  ];
  // Typing in the 「拒绝，并告诉 Claude」 row takes the digits (the keys skip an input).
  useNumberKeys(focus && !busy, actions);
  let n = 0;
  return (
    <Shell
      icon={<ShieldQuestionMark size={14} strokeWidth={1.6} />}
      title={`允许 ${r.tool_name}？`}
      status="等待审批"
      source={r.layout_source}
      agent={r.agent_id}
      testid="permission-card"
    >
      <ToolInput r={r} sessionId={c.sessionId} />
      <div className="flex flex-col gap-1.5">
        <OptionRow
          n={++n}
          label={label("allow", "允许")}
          primary
          disabled={busy}
          onClick={() => void answer("allow")}
          testid="allow"
        />
        {always && (
          <OptionRow
            n={++n}
            label={always.label}
            disabled={busy}
            onClick={() => void answer("allow_always")}
            testid="allow-always"
          />
        )}
        <OptionRow
          n={++n}
          label={label("deny", "拒绝")}
          disabled={busy}
          onClick={() => void answer("deny")}
          testid="deny"
        />
        <form
          className="flex items-center gap-2 rounded-md border border-border px-2.5 py-1 text-[12px] focus-within:border-accent"
          onSubmit={(e) => {
            e.preventDefault();
            if (feedback.trim() && !busy) void answer("deny", feedback.trim());
          }}
        >
          <span className="shrink-0 text-secondary">拒绝，并告诉 Claude：</span>
          <input
            value={feedback}
            onChange={(e) => setFeedback(e.target.value)}
            placeholder="要 Claude 怎么改（本轮继续），回车发送"
            className="h-6 min-w-0 flex-1 bg-transparent outline-none placeholder:text-secondary"
            data-testid="deny-message"
          />
        </form>
      </div>
      <CardFoot error={error} />
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
    <Shell
      icon={<ListChecks size={14} strokeWidth={1.6} />}
      title={r.title}
      status="等待选择"
      source={r.layout_source}
      testid="choice-card"
    >
      <div className="flex flex-col gap-1.5">
        {r.options.map((o, i) => (
          <OptionRow
            key={o.label}
            n={i + 1}
            label={o.label}
            desc={o.description}
            primary={i === 0}
            disabled={busy}
            onClick={() => pick(o.label)}
            testid="choice-option"
          />
        ))}
      </div>
      <CardFoot error={error} />
    </Shell>
  );
}

/** Focus is nowhere in particular (on the page, or on the panel), so it may be moved. */
const focusFree = () => {
  const a = document.activeElement;
  return !a || a === document.body || !!a.closest("[data-connect-panel]");
};

/** Rewind cards (`conn|request_id`) already shown once: focus moves to a card only when it first appears. */
const shownRewinds = new Set<string>();

/**
 * The rewind's confirm page (`rewind`): the prompt, what happens to the code, the restores it offers (number keys as
 * on a choice card) and 「取消」 (Esc, `answer_rewind` cancel).
 */
function RewindCard({ r, c, focus }: { r: RewindRequest; c: ConnState; focus: boolean }) {
  const answerRewind = useConnect((s) => s.answerRewind);
  const [busy, setBusy] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const options = REWIND_ORDER.filter((o) => r.options.some((x) => x.option === o));
  const answer = async (option: RewindOption | null) => {
    setBusy(true);
    await answerRewind(c.conn, r, option);
    setBusy(false);
    // The prompt is back in the input box: ready to edit and send.
    if (option && option !== "code" && focusFree())
      document.querySelector<HTMLElement>("[data-testid=connect-input]")?.focus();
  };
  // Just asked for (focus still on the 「回退」 clicked, or nowhere): Esc and the number keys reach the card. The card
  // itself takes focus, never one of its options; not when it shows again (back on the Session), nor while typing.
  const conn = c.conn;
  const id = r.request_id;
  useEffect(() => {
    const key = `${conn}|${id}`;
    if (shownRewinds.has(key)) return;
    shownRewinds.add(key);
    const a = document.activeElement;
    if (!a || a === document.body || a.closest("[data-testid=rewind-button]")) box.current?.focus();
  }, [conn, id]);
  useNumberKeys(
    focus && !busy,
    options.map((o) => () => void answer(o)),
  );
  return (
    <div ref={box} tabIndex={-1} className="outline-none" data-testid="rewind-card-box">
      <Shell
        icon={<RotateCcw size={14} strokeWidth={1.6} />}
        title="回退到这条提示词之前"
        status="等待确认"
        testid="rewind-card"
      >
        <p
          data-testid="rewind-quote"
          className="m-0 line-clamp-3 border-l-2 border-border pl-2.5 break-words whitespace-pre-wrap text-text/80"
        >
          {r.text}
        </p>
        <p data-testid="rewind-code-line" className="m-0">
          {codeLine(r.code)}
        </p>
        {r.warning && (
          <p data-testid="rewind-warning" className="m-0 text-tag-text">
            {REWIND_WARNING}
          </p>
        )}
        {/* None drawn picked: nothing that restores code is a default. */}
        <div className="flex flex-col gap-1.5">
          {options.map((o, i) => (
            <OptionRow
              key={o}
              n={i + 1}
              label={REWIND_LABELS[o]}
              desc={REWIND_DESC[o]}
              disabled={busy}
              onClick={() => void answer(o)}
              testid={`rewind-${o}`}
            />
          ))}
        </div>
        <CardFoot>
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => void answer(null)}
            data-testid="rewind-cancel"
          >
            取消（Esc）
          </Button>
        </CardFoot>
      </Shell>
    </div>
  );
}
