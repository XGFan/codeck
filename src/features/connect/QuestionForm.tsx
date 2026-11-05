import { Check } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import { isComposingKey } from "@/lib/hotkeys";
import { otherChosen, questionAnswers, togglePick } from "./answers";
import type { ConnState } from "./conn";
import type { AskQuestion, QuestionEvent } from "./protocol";
import { useAnswer } from "./useAnswer";

/** The questions of an AskUserQuestion to pick and submit, styled as the transcript's 「向你提问」 card. */
export function QuestionForm({ r, c }: { r: QuestionEvent; c: ConnState }) {
  const { busy, error, send } = useAnswer(c);
  const qs = Array.isArray(r.questions) ? r.questions : [];
  const [picked, setPicked] = useState<string[][]>(() => qs.map(() => []));
  const [other, setOther] = useState<string[]>(() => qs.map(() => ""));
  // The answer is either the options picked or the written one, never both: typing in 其他 drops
  // the picked options, picking an option sets the written text aside (kept, greyed).
  const [useOther, setUseOther] = useState<boolean[]>(() => qs.map(() => false));
  const otherOn = (i: number) => otherChosen(other, useOther, i);
  const ready = qs.every((_, i) => picked[i]?.length || otherOn(i));
  const submit = () =>
    void send({
      type: "answer_question",
      request_id: r.request_id,
      answers: questionAnswers(qs, picked, other, useOther),
    });
  const at = <T,>(xs: T[], i: number, v: T) => xs.map((x, j) => (j === i ? v : x));
  const toggle = (i: number, q: AskQuestion, label: string) => {
    setUseOther((u) => at(u, i, false));
    setPicked((p) => p.map((x, j) => (j === i ? togglePick(x, label, !!q.multiSelect) : x)));
  };
  const chooseOther = (i: number) => {
    setUseOther((u) => at(u, i, true));
    setPicked((p) => at(p, i, []));
  };
  return (
    <div className="flex flex-col gap-3" data-testid="question-form">
      {qs.map((q, i) => (
        <fieldset key={i} className="m-0 flex flex-col gap-1.5 border-0 p-0">
          {/* One paragraph: a long question wraps back to the left edge, under the tag. */}
          <legend className="mb-1.5 p-0 text-[12px] leading-[1.6]">
            {q.header && (
              <span className="mr-2 inline-block rounded bg-selection px-1.5 text-[11px] leading-[1.6] whitespace-nowrap text-accent">
                {q.header}
              </span>
            )}
            <span className="font-medium">{q.question}</span>
            {q.multiSelect && <span className="ml-2 text-[11px] whitespace-nowrap text-secondary">多选</span>}
          </legend>
          {q.options.map((o) => {
            const on = !!picked[i]?.includes(o.label);
            return (
              <button
                type="button"
                key={o.label}
                aria-pressed={on}
                data-testid="question-option"
                data-card-answer
                onClick={() => toggle(i, q, o.label)}
                className={cn(
                  "flex items-start gap-2 rounded-md border px-2.5 py-1.5 text-left text-[12px] outline-none focus-visible:border-accent",
                  on ? "border-accent/50 bg-selection" : "border-border hover:bg-selection/60",
                )}
              >
                <Mark on={on} multi={!!q.multiSelect} className="mt-0.5" />
                <span>
                  <span className="font-medium">{o.label}</span>
                  {o.description && <span className="ml-2 text-secondary">{o.description}</span>}
                </span>
              </button>
            );
          })}
          <label
            data-testid="question-other-row"
            data-on={otherOn(i) || undefined}
            className={cn(
              "flex items-center gap-2 rounded-md border px-2.5 py-1 text-[12px] focus-within:border-accent",
              otherOn(i) ? "border-accent/50 bg-selection" : "border-border",
            )}
          >
            <Mark on={otherOn(i)} multi={!!q.multiSelect} />
            <span className="shrink-0 text-secondary">其他：</span>
            <input
              value={other[i] ?? ""}
              onChange={(e) => {
                setOther((p) => at(p, i, e.target.value));
                chooseOther(i);
              }}
              // Back to the written answer by clicking into it.
              onFocus={() => other[i]?.trim() && chooseOther(i)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !isComposingKey(e.nativeEvent) && ready && !busy) submit();
              }}
              placeholder="自己写答案"
              className={cn(
                "h-6 min-w-0 flex-1 bg-transparent outline-none placeholder:text-secondary",
                !otherOn(i) && other[i]?.trim() && "text-secondary line-through decoration-secondary/60",
              )}
              data-testid="question-other"
            />
          </label>
        </fieldset>
      ))}
      <div className="flex items-center gap-2">
        <Button size="sm" disabled={busy || !ready} onClick={submit} data-testid="submit-answer">
          提交
        </Button>
        {error && <span className="text-[12px] text-error">{error}</span>}
      </div>
    </div>
  );
}

/** The choice's mark: a radio dot for one answer, a check box when several can be picked. */
function Mark({ on, multi, className }: { on: boolean; multi: boolean; className?: string }) {
  return (
    <span
      aria-hidden
      className={cn(
        "flex size-3.5 shrink-0 items-center justify-center border",
        className,
        multi ? "rounded-[3px]" : "rounded-full",
        on ? "border-accent bg-accent text-white" : "border-secondary/50 bg-ground",
      )}
    >
      {on && (multi ? <Check size={10} strokeWidth={3} /> : <span className="size-1.5 rounded-full bg-white" />)}
    </span>
  );
}
