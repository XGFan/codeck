// Rewind in a Connected Session (ccrpc `rewind` / `answer_rewind`): which prompts can be rewound to, and the words
// of the confirm card and the transcript's marker.
import type { BranchPoint, Node } from "@/ipc/bindings";
import type { RewindOption } from "./protocol";

/** The card's buttons, in this order (only those the confirm page offers). */
export const REWIND_LABELS: Record<RewindOption, string> = {
  both: "恢复代码与对话",
  conversation: "只恢复对话",
  code: "只恢复代码",
};
export const REWIND_ORDER: RewindOption[] = ["both", "conversation", "code"];

/** The confirm page's `⚠` warning, said in Chinese. */
export const REWIND_WARNING = "恢复的文件整份覆盖，之后的改动都会丢失；Bash 对其他文件的改动不会撤销。";

const REMOVED = "这条提示词及之后的对话会被移除";

/**
 * The confirm page's sentence on the code, in Chinese: `The code will be unchanged.`, `The code will be restored
 * <diff> in <files>.`; another sentence as it is.
 */
export function codeLine(code: string | null): string {
  if (code == null) return REMOVED;
  const s = code.trim();
  if (/^The code will be unchanged\.?$/.test(s)) return `${REMOVED}；代码不变`;
  const m = /^The code will be restored (.+?) in (.+?)\.?$/.exec(s);
  if (m) return `${REMOVED}；代码会恢复：${m[2]}（${m[1]!.replace(/-(?=\d)/g, "−")}）`;
  return s;
}

/** The divider above the rewound prompt. */
export const rewoundLabel = (withCode: boolean) =>
  `${withCode ? "已回退到这里，代码已恢复" : "已回退到这里"} · 发出新提示词后，下面这段成为另一个 Branch`;

/**
 * The prompts and commands the person typed on the Main Line part of a shown path, in order, and whether the whole path is the
 * Main Line (it leaves it at a branch head whose shown option is another Branch).
 */
export function mainLinePrompts(nodes: Node[], branchPoints: BranchPoint[]): { ids: string[]; wholeMain: boolean } {
  const offMain = new Set(
    branchPoints
      .filter((b) => b.options.some((o) => o.headId === b.selectedHeadId && !o.isMainLine))
      .map((b) => b.selectedHeadId),
  );
  const ids: string[] = [];
  for (const n of nodes) {
    if (offMain.has(n.id)) return { ids, wholeMain: false };
    // Typed prompts and slash / `!` commands: the terminal's Rewind picker lists them all.
    if (n.body.kind === "userPrompt" && (n.body.origin.kind === "human" || n.body.origin.kind === "command"))
      ids.push(n.id);
  }
  return { ids, wholeMain: true };
}

/** The prompts offering 「回退」: the Main Line's, up to the one already rewound to (from it on, it is gone). */
export function rewindable(ids: string[], rewound: string | null): string[] {
  const at = rewound ? ids.indexOf(rewound) : -1;
  return at >= 0 ? ids.slice(0, at) : ids;
}
