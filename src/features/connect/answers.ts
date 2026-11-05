import type { AskQuestion, PermissionRequestEvent, ResponseMsg } from "./protocol";

/** What to say when ccrpc refuses an answer. */
export function answerError(error: ResponseMsg["error"]): string {
  return error?.code === "not_offered"
    ? "这次审批没有“总是允许”选项"
    : `作答失败：${error?.message ?? error?.code ?? ""}`;
}

/**
 * The words on a permission card's button for `meaning`: the dialog's own for a learned layout (its meanings come
 * from a model, so a wrongly labelled option is visible before it is pressed), and for an allow that says more than
 * a plain `Yes…` (computer use's `Allow for this session (4 apps)`: on a narrow screen apps are left out of the
 * grant); otherwise `fallback`.
 */
export function permissionLabel(
  r: Pick<PermissionRequestEvent, "options" | "layout_source">,
  meaning: string,
  fallback: string,
): string {
  const own = r.options?.find((o) => o.meaning === meaning)?.label;
  if (!own) return fallback;
  if (r.layout_source === "learned") return own;
  return meaning === "allow" && !/^yes\b/i.test(own) ? own : fallback;
}

/** Picking an option: toggles it among the others when several can be picked, otherwise it is the only one. */
export function togglePick(picked: string[], label: string, multi: boolean): string[] {
  const on = picked.includes(label);
  return multi ? (on ? picked.filter((l) => l !== label) : [...picked, label]) : on ? [] : [label];
}

/**
 * The `answers` of `answer_question`, one per question: on a single-select question the written answer when 其他 is
 * chosen and not blank, otherwise the option picked; on a multi-select one the options picked and the written answer,
 * either or both.
 */
export function questionAnswers(qs: AskQuestion[], picked: string[][], other: string[], useOther: boolean[]) {
  return qs.map((q, i) => {
    const options = picked[i] ?? [];
    if (!otherChosen(other, useOther, i)) return { options };
    const written = other[i]!.trim();
    return q.multiSelect && options.length ? { options, other: written } : { other: written };
  });
}

/** Question `i` is answered by its written text: 其他 is chosen and something is written. */
export const otherChosen = (other: string[], useOther: boolean[], i: number) => !!useOther[i] && !!other[i]?.trim();
