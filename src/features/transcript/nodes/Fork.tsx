import { format } from "date-fns";
import { GitFork } from "lucide-react";
import type { InheritedRange } from "@/ipc/bindings";
import { useUi } from "@/state/ui";
import { INHERITED_KEY } from "../keys";

/** "Fork 自 <Origin Session>（…）" with the expand/collapse toggle for the inherited prefix. */
export function InheritedBanner({
  range,
  count,
  expanded,
  lastTimestampMs,
}: {
  range: InheritedRange | null;
  count: number;
  expanded: boolean;
  lastTimestampMs: number | null;
}) {
  const toggle = useUi((s) => s.toggleExpanded);
  const selectSession = useUi((s) => s.selectSession);
  const when = [lastTimestampMs != null ? format(lastTimestampMs, "M月d日 HH:mm") : null, `第 ${count} 条消息之后`]
    .filter(Boolean)
    .join("，");
  return (
    <div
      data-testid="fork-banner"
      className="flex flex-wrap items-center gap-2.5 rounded-[10px] border border-accent/30 bg-selection/50 px-3.5 py-2.5"
    >
      <GitFork size={16} strokeWidth={1.6} className="shrink-0 text-accent" aria-hidden />
      <span className="min-w-[200px] flex-1">
        Fork 自{" "}
        {range ? (
          <button
            type="button"
            onClick={() => selectSession(range.originSessionId)}
            className="text-accent hover:underline"
          >
            {range.originTitle ?? range.originSessionId}
          </button>
        ) : (
          "另一个 Session"
        )}
        <span className="text-secondary">（{when}）</span>
      </span>
      <button
        type="button"
        data-testid="toggle-inherited"
        aria-expanded={expanded}
        onClick={() => toggle(INHERITED_KEY)}
        className="h-[26px] rounded-md border border-accent/30 px-2.5 text-[12px] text-accent hover:bg-selection"
      >
        {expanded ? `收起继承的 ${count} 条消息` : `展开继承的 ${count} 条消息`}
      </button>
    </div>
  );
}

export function InheritedDivider() {
  return (
    <div data-testid="fork-divider" className="flex items-center gap-2.5 text-[12px] text-secondary">
      <span className="h-px flex-1 bg-border" />
      <span>以下为本 Session 新增内容</span>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}
