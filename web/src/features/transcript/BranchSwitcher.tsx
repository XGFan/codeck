import { ChevronLeft, ChevronRight } from "lucide-react";
import type { BranchPoint } from "@/ipc/bindings";
import { useUi } from "@/state/ui";

/** `‹ i/n ›` before a branch head; picking an option stores `{anchorKey, headId}` and the transcript refetches. */
export function BranchSwitcher({ point }: { point: BranchPoint }) {
  const setBranchChoice = useUi((s) => s.setBranchChoice);
  const { options } = point;
  const i = Math.max(
    0,
    options.findIndex((o) => o.headId === point.selectedHeadId),
  );
  const prev = options[i - 1];
  const next = options[i + 1];
  const pick = (headId: string) => setBranchChoice({ anchorKey: point.anchorKey, headId });

  return (
    <span data-testid="branch-switcher" className="flex items-center gap-0.5 text-text/80">
      <button
        type="button"
        aria-label="上一个分支"
        title={prev ? branchTitle(prev.preview, prev.isMainLine) : undefined}
        disabled={!prev}
        onClick={() => prev && pick(prev.headId)}
        className="flex size-[22px] items-center justify-center rounded hover:bg-selection disabled:text-secondary/50 disabled:hover:bg-transparent"
      >
        <ChevronLeft size={11} strokeWidth={2} aria-hidden />
      </button>
      <span data-testid="branch-index" className="tabular-nums">
        {i + 1} / {options.length}
      </span>
      <button
        type="button"
        aria-label="下一个分支"
        title={next ? branchTitle(next.preview, next.isMainLine) : undefined}
        disabled={!next}
        onClick={() => next && pick(next.headId)}
        className="flex size-[22px] items-center justify-center rounded hover:bg-selection disabled:text-secondary/50 disabled:hover:bg-transparent"
      >
        <ChevronRight size={11} strokeWidth={2} aria-hidden />
      </button>
    </span>
  );
}

const branchTitle = (preview: string, mainLine: boolean) => `${mainLine ? "Main Line · " : ""}${preview.slice(0, 80)}`;
