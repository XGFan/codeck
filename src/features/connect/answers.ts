import type { AskQuestion, ResponseMsg } from "./protocol";

/** What to say when ccrpc refuses an answer. */
export function answerError(error: ResponseMsg["error"]): string {
  return error?.code === "not_offered"
    ? "这次审批没有“总是允许”选项"
    : `作答失败：${error?.message ?? error?.code ?? ""}`;
}

/** Picking an option: toggles it among the others when several can be picked, otherwise it is the only one. */
export function togglePick(picked: string[], label: string, multi: boolean): string[] {
  const on = picked.includes(label);
  return multi ? (on ? picked.filter((l) => l !== label) : [...picked, label]) : on ? [] : [label];
}

/**
 * The `answers` of `answer_question`, one per question: the written answer when 其他 is chosen and not blank,
 * otherwise the options picked.
 */
export function questionAnswers(qs: AskQuestion[], picked: string[][], other: string[], useOther: boolean[]) {
  return qs.map((_, i) => (otherChosen(other, useOther, i) ? { other: other[i]!.trim() } : { options: picked[i] }));
}

/** Question `i` is answered by its written text: 其他 is chosen and something is written. */
export const otherChosen = (other: string[], useOther: boolean[], i: number) => !!useOther[i] && !!other[i]?.trim();
