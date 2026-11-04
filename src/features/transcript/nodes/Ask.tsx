import { Check, MessageCircleQuestionMark } from "lucide-react";
import type { Node } from "@/ipc/bindings";
import { QuestionForm, useQuestionRequest } from "@/features/drive";
import { cn } from "@/lib/cn";
import type { RoleInfo, ToolCallBlock } from "../grouping";
import { parseJson } from "../tools/data";
import { clock } from "../util";
import { Anchor } from "./Anchor";
import { AssistantSegment } from "./Assistant";

interface Question {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options?: { label: string; description?: string; preview?: string }[];
}

interface AskResult {
  questions?: Question[];
  answers?: Record<string, string>;
  annotations?: Record<string, { notes?: string }>;
}

const CLARIFY = "The user wants to clarify these questions.";

/**
 * Answers given before the user picked "Chat about this": the call is rejected and the result
 * text lists each question as `- "question"`, followed by `  Answer: …` / `  User notes: …`
 * lines or `  (No answer provided)`.
 */
export function clarifyAnswers(text: string): AskResult {
  const answers: Record<string, string> = {};
  const annotations: Record<string, { notes?: string }> = {};
  const asked = text.split("Questions asked:")[1]?.split("\n\nNote:")[0] ?? "";
  for (const m of asked.matchAll(/^- "([\s\S]*?)"\n((?: {2}.*(?:\n|$))*)/gm)) {
    const answer = /^ {2}Answer: (.*)$/m.exec(m[2])?.[1];
    const notes = /^ {2}User notes: (.*)$/m.exec(m[2])?.[1];
    if (answer) answers[m[1]] = answer;
    if (notes) annotations[m[1]] = { notes };
  }
  return { answers, annotations };
}

/** AskUserQuestion: one card with every question and option; the user's picks are marked in place. */
export function AskRow({ node, call, role }: { node: Node; call: ToolCallBlock; role: RoleInfo | null }) {
  const result = call.result;
  // "Chat about this" is recorded as a rejection, but it is the user's choice, not a failure.
  const clarify = result?.isError === true && result.text.includes(CLARIFY);
  const failed = result?.isError === true && !clarify;
  const extra = parseJson<AskResult>(result?.extraJson) ?? (clarify ? clarifyAnswers(result.text) : null);
  const questions = extra?.questions ?? parseJson<{ questions?: Question[] }>(call.inputJson)?.questions ?? [];
  const answers = extra?.answers ?? {};
  const notes = extra?.annotations ?? {};
  const answered = questions.some((q) => answers[q.question] || notes[q.question]?.notes);
  // Answers keyed by something other than the question text: show the raw result instead.
  const fallback = result && !failed && !clarify && !answered ? result.text.trim() : "";
  const at = result?.timestampMs != null ? `你 · ${clock(result.timestampMs)} ` : "";
  const status = !result ? "等待回答" : failed ? "未回答" : clarify ? `${at}选择先聊聊` : `${at}已回答`;
  // Driven and waiting on this question: answered right here.
  const pending = useQuestionRequest(call.toolUseId);
  const answering = !result && pending;
  return (
    <AssistantSegment role={role}>
      <Anchor nodeId={node.id} toolUseId={call.toolUseId} className="w-full">
        <div
          data-testid="ask"
          data-failed={failed || undefined}
          data-answering={answering ? "" : undefined}
          className={cn("w-full overflow-hidden rounded-lg border", failed ? "border-error/30" : answering ? "border-live-waiting/60 shadow-sm" : "border-border")}
        >
          <div className="flex items-center gap-2 border-b border-border bg-list px-2.5 py-1.5 text-[12px] text-text/80">
            <MessageCircleQuestionMark size={14} strokeWidth={1.6} className="text-accent" aria-hidden />
            <span className="font-semibold">向你提问</span>
            {questions.length > 1 && <span className="text-secondary">{questions.length} 个问题</span>}
            <span data-testid="ask-status" className={cn("ml-auto", failed ? "text-error" : answering ? "text-live-waiting" : "text-secondary")}>
              {status}
            </span>
          </div>
          <div className="flex flex-col gap-3 bg-ground px-3 py-2.5">
            {answering ? (
              <QuestionForm r={answering.r} c={answering.c} />
            ) : (
              questions.map((q, i) => <QuestionView key={`${i}|${q.question}`} q={q} answer={answers[q.question]} note={notes[q.question]?.notes} />)
            )}
            {fallback && <div className="text-[12px] whitespace-pre-wrap text-secondary">{fallback}</div>}
          </div>
          {failed && <div className="border-t border-error/30 bg-error-bg px-3 py-1.5 text-[12px] whitespace-pre-wrap text-error">{result.text}</div>}
        </div>
      </Anchor>
    </AssistantSegment>
  );
}

function QuestionView({ q, answer, note }: { q: Question; answer: string | undefined; note: string | undefined }) {
  const options = q.options ?? [];
  const labels = new Set(options.map((o) => o.label));
  // Multi-select answers join the picked labels with ", "; whatever is not a label was typed in "Other".
  const picked = !answer ? [] : q.multiSelect && !labels.has(answer) ? answer.split(/,\s*/).filter(Boolean) : [answer];
  const custom = picked.filter((p) => !labels.has(p)).join(", ");
  return (
    <div data-testid="ask-question" className="flex flex-col gap-1.5">
      {/* One paragraph: a long question wraps back to the left edge, under the tag. */}
      <div data-testid="ask-legend" className="text-[12px] leading-[1.6]">
        {q.header && <span className="mr-2 inline-block rounded bg-selection px-1.5 text-[11px] leading-[1.6] whitespace-nowrap text-accent">{q.header}</span>}
        <span className="font-medium">{q.question}</span>
        {q.multiSelect && <span className="ml-2 text-[11px] whitespace-nowrap text-secondary">多选</span>}
      </div>
      {options.map((o, i) => {
        const on = picked.includes(o.label);
        return (
          <div
            key={`${i}|${o.label}`}
            data-chosen={on || undefined}
            className={cn("flex flex-col gap-1 rounded-md border px-2.5 py-1.5 text-[12px]", on ? "border-accent/50 bg-selection" : "border-border")}
          >
            <div className="flex items-start gap-2">
              <span className="mt-0.5 flex size-3.5 shrink-0 items-center justify-center text-accent">{on && <Check size={13} strokeWidth={2.2} />}</span>
              <span className={cn(answer && !on && "text-secondary")}>
                <span className="font-medium">{o.label}</span>
                {o.description && <span className="ml-2 text-secondary">{o.description}</span>}
              </span>
            </div>
            {on && o.preview && <pre className="m-0 ml-5.5 overflow-x-auto font-mono text-[11.5px] leading-snug text-text/80">{o.preview}</pre>}
          </div>
        );
      })}
      {custom && (
        <div data-chosen className="rounded-md border border-accent/50 bg-selection px-2.5 py-1.5 text-[12px]">
          <span className="text-secondary">其他：</span>
          {custom}
        </div>
      )}
      {note && <div className="text-[12px] whitespace-pre-wrap text-secondary">备注：{note}</div>}
    </div>
  );
}
