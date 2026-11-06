import { cn } from "@/lib/cn";
import { cells } from "./cells";

interface PreviewOption {
  label: string;
  preview?: string;
}

/** Whether any option of a question carries a preview (an ASCII mockup, code, a diagram). */
export function hasPreview(options: readonly PreviewOption[]): boolean {
  return options.some((o) => !!o.preview);
}

/**
 * The preview of option `active`, beside the options as in Claude Code. Every preview is laid out in the same box
 * (the others hidden), so the box keeps the size of the largest and the options do not move as the shown one changes.
 */
export function OptionPreview({ options, active }: { options: readonly PreviewOption[]; active: number }) {
  return (
    <div
      data-testid="option-preview"
      data-option={active}
      className="grid max-w-full min-w-0 flex-[0_1_auto] overflow-x-auto rounded-md border border-border bg-list px-2.5 py-1.5"
    >
      {options.map((o, i) => (
        <pre
          key={`${i}|${o.label}`}
          aria-hidden={i !== active || undefined}
          className={cn(
            "col-start-1 row-start-1 m-0 font-mono text-[11.5px] leading-[1.25] text-text/80",
            i !== active && "invisible",
          )}
        >
          {o.preview ? <Grid text={o.preview} /> : <span className="font-sans text-secondary">此项没有预览</span>}
        </pre>
      ))}
    </div>
  );
}

/** Text on the terminal's grid: each non-ASCII character in a box one or two columns wide. */
function Grid({ text }: { text: string }) {
  return cells(text).map((r, i) =>
    r.w === 0 ? (
      r.text
    ) : (
      <span key={i} className="inline-block text-center" style={{ width: `${r.w}ch` }}>
        {r.text}
      </span>
    ),
  );
}
