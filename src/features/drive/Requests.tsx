import { Sparkles } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { PendingText, toolKey, ToolCallView } from "@/features/transcript/tools";
import type { ToolCall } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { isComposingKey } from "@/lib/hotkeys";
import { useUi } from "@/state/ui";
import type { ConnState, Request } from "./conn";
import type { ChoiceEvent, LayoutSource, PermissionRequestEvent, QuestionEvent } from "./protocol";
import { useDrive } from "./store";

/** Cards for what the Session waits on, main line first. */
export function Requests({ c }: { c: ConnState }) {
  const reqs = [...c.requests].sort((a, b) => Number(isSub(a)) - Number(isSub(b)));
  if (reqs.length === 0) {
    // Waiting with nothing to show yet: the analyzer may be looking at the screen.
    if (c.state === "waiting" && c.waiting?.kind !== "question")
      return (
        <div className="rounded-lg border border-border px-3 py-2 text-[12px] text-secondary" data-testid="analyzing">
          正在识别界面…
        </div>
      );
    return null;
  }
  return (
    <div className="flex max-h-[45vh] flex-col gap-2 overflow-y-auto" data-testid="drive-requests">
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
      return <QuestionCard r={r} c={c} />;
    case "choice":
      return <ChoiceCard r={r} c={c} focus={focus} />;
    default:
      return (
        <Shell title={r.type === "dialog" ? "Claude Code 停在一个 ccrpc 认不出的界面" : `认不出的屏幕（${r.context}）`} testid="screen-card">
          <pre className="max-h-64 overflow-auto rounded bg-terminal p-2 font-mono text-[11px] leading-[1.45] text-[#e4e4e6]">
            {trimLines(r.lines).join("\n")}
          </pre>
          <p className="mt-2 text-[12px] text-secondary">需在 Mac Client 的终端里处理。</p>
        </Shell>
      );
  }
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
        <span className="size-2 shrink-0 rounded-full bg-live-waiting" />
        <h3 className="m-0 min-w-0 flex-1 truncate text-[13px] font-semibold">{title}</h3>
        {agent && <span className="rounded bg-code px-1.5 text-[11px] text-secondary">Subagent {agent.slice(0, 8)}</span>}
        {source === "learned" && (
          <span className="flex items-center gap-1 rounded bg-selection px-1.5 text-[11px] text-accent" title="分析器学到的布局" data-testid="ai-badge">
            <Sparkles size={11} aria-hidden /> AI 识别
          </span>
        )}
      </div>
      {children}
    </section>
  );
}

function useAnswer(c: ConnState) {
  const command = useDrive((s) => s.command);
  const dropRequest = useDrive((s) => s.dropRequest);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const send = async (msg: Record<string, unknown>) => {
    setBusy(true);
    setError(null);
    const r = await command(c.conn, msg);
    setBusy(false);
    // Answered: off screen now, so the next card gets the keys (ccrpc ends it on the tool's result).
    if (r.success) dropRequest(c.conn, String(msg.request_id));
    else setError(r.error?.code === "not_offered" ? "这次审批没有“总是允许”选项" : `作答失败：${r.error?.message ?? r.error?.code ?? ""}`);
  };
  return { busy, error, send };
}

/** Number keys pick the n-th button of the first card while focus is not in an editable field and no dialog is open. */
function useNumberKeys(enabled: boolean, actions: (() => void)[]) {
  const ref = useRef(actions);
  ref.current = actions;
  useEffect(() => {
    if (!enabled) return;
    const on = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "TEXTAREA" || el.tagName === "INPUT" || el.tagName === "SELECT" || el.isContentEditable)) return;
      if (isComposingKey(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      const ui = useUi.getState();
      if (ui.searchOpen || ui.statsOpen || ui.settingsOpen || document.querySelector("[role=dialog], [data-radix-popper-content-wrapper]")) return;
      const n = Number(e.key);
      if (n >= 1 && n <= ref.current.length) {
        e.preventDefault();
        ref.current[n - 1]!();
      }
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [enabled]);
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
  const nodeId = `drive:${r.request_id}`;
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
  const answer = (decision: string, message?: string) =>
    send({ type: "answer_permission", request_id: r.request_id, decision, ...(message ? { message } : {}) });
  const actions = [() => void answer("allow"), ...(always ? [() => void answer("allow_always")] : []), () => void answer("deny")];
  useNumberKeys(focus && feedback == null && !busy, actions);
  let n = 0;
  return (
    <Shell title={`允许 ${r.tool_name}？`} source={r.layout_source} agent={r.agent_id} testid="permission-card">
      <ToolInput r={r} sessionId={c.sessionId} />
      <div className="mt-2.5 flex flex-wrap items-center gap-1.5">
        <Button size="sm" disabled={busy} onClick={() => void answer("allow")} data-testid="allow">
          {++n}. 允许
        </Button>
        {always && (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void answer("allow_always")} title={always.label} data-testid="allow-always" className="max-w-[28rem]">
            <span className="truncate">
              {++n}. {always.label}
            </span>
          </Button>
        )}
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void answer("deny")} data-testid="deny">
          {++n}. 拒绝
        </Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => setFeedback(feedback == null ? "" : null)} data-testid="deny-with-message">
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

function QuestionCard({ r, c }: { r: QuestionEvent; c: ConnState }) {
  const { busy, error, send } = useAnswer(c);
  const qs = Array.isArray(r.questions) ? r.questions : [];
  const [picked, setPicked] = useState<string[][]>(() => qs.map(() => []));
  const [other, setOther] = useState<string[]>(() => qs.map(() => ""));
  const ready = qs.every((_, i) => picked[i]?.length || other[i]?.trim());
  const submit = () =>
    void send({
      type: "answer_question",
      request_id: r.request_id,
      answers: qs.map((_, i) => (other[i]?.trim() && !picked[i]?.length ? { other: other[i]!.trim() } : { options: picked[i] })),
    });
  return (
    <Shell title={qs.length > 1 ? `Claude 有 ${qs.length} 个问题` : "Claude 在提问"} agent={r.agent_id} testid="question-card">
      <div className="space-y-3">
        {qs.map((q, i) => (
          <fieldset key={i} className="m-0 border-0 p-0">
            <legend className="mb-1 text-[13px]">
              {q.header && <span className="mr-1.5 rounded bg-code px-1 text-[11px] text-secondary">{q.header}</span>}
              {q.question}
            </legend>
            <div className="flex flex-wrap gap-1.5">
              {q.options.map((o) => {
                const on = picked[i]?.includes(o.label);
                return (
                  <button
                    type="button"
                    key={o.label}
                    title={o.description}
                    aria-pressed={on}
                    data-testid="question-option"
                    onClick={() =>
                      setPicked((p) =>
                        p.map((x, j) => (j !== i ? x : q.multiSelect ? (on ? x.filter((l) => l !== o.label) : [...x, o.label]) : on ? [] : [o.label])),
                      )
                    }
                    className={cn("rounded-md border px-2 py-1 text-left text-[12px]", on ? "border-accent bg-selection" : "border-border hover:bg-selection")}
                  >
                    <span className="block">{o.label}</span>
                    {o.description && <span className="block text-[11px] text-secondary">{o.description}</span>}
                  </button>
                );
              })}
            </div>
            <input
              value={other[i] ?? ""}
              onChange={(e) => setOther((p) => p.map((x, j) => (j === i ? e.target.value : x)))}
              placeholder="其他（自己写答案）"
              className="mt-1.5 h-7 w-full rounded-md border border-border bg-ground px-2 text-[12px] outline-none focus:border-accent"
              data-testid="question-other"
            />
          </fieldset>
        ))}
      </div>
      <div className="mt-2.5 flex items-center gap-2">
        <Button size="sm" disabled={busy || !ready} onClick={submit} data-testid="submit-answer">
          提交
        </Button>
        {error && <span className="text-[12px] text-error">{error}</span>}
      </div>
    </Shell>
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
