import { CircleAlert, X } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * A notice above the input box (Codex style): icon, a title, one line saying more, then an action and a close
 * button at the right. `tone` colors the icon (and the title for an error).
 */
export function NoticeCard({
  tone = "info",
  icon,
  title,
  desc,
  action,
  onClose,
  testId,
  children,
}: {
  tone?: "info" | "wait" | "error";
  icon: ReactNode;
  title: ReactNode;
  desc?: ReactNode;
  /** A button: `primary` is filled, else outlined. */
  action?: {
    label: string;
    onClick: () => void;
    primary?: boolean;
    disabled?: boolean;
    title?: string;
    testId?: string;
  };
  onClose?: () => void;
  testId?: string;
  /** Shown under the row (more text the card can unfold). */
  children?: ReactNode;
}) {
  return (
    <div
      data-testid={testId}
      className="flex w-full flex-col rounded-2xl border border-border bg-ground py-2.5 pr-3 pl-4 shadow-[0_2px_10px_rgba(0,0,0,0.04)]"
    >
      <div className="flex w-full items-center gap-3">
        <span
          className={cn(
            "flex shrink-0",
            tone === "error" ? "text-error" : tone === "wait" ? "text-live-waiting" : "text-secondary",
          )}
        >
          {icon}
        </span>
        <div className="flex min-w-0 flex-1 flex-col">
          <span className={cn("truncate text-[13px] font-semibold", tone === "error" && "text-error")}>{title}</span>
          {desc && <span className="truncate text-[12.5px] text-secondary">{desc}</span>}
        </div>
        {action && (
          <>
            <span className="w-px self-stretch bg-border" aria-hidden />
            <button
              type="button"
              onClick={action.onClick}
              disabled={action.disabled}
              title={action.title}
              data-testid={action.testId}
              className={cn(
                "h-7 shrink-0 rounded-full px-3 text-[12.5px] disabled:opacity-40",
                action.primary ? "bg-text text-ground hover:opacity-90" : "border border-border hover:bg-selection/60",
              )}
            >
              {action.label}
            </button>
          </>
        )}
        {onClose && (
          <button
            type="button"
            aria-label="关闭"
            onClick={onClose}
            className="flex size-6 shrink-0 items-center justify-center rounded-md text-secondary hover:bg-selection/60 hover:text-text"
          >
            <X size={14} aria-hidden />
          </button>
        )}
      </div>
      {children}
    </div>
  );
}

/** An error message as a notice: what failed as the title, the reason (after 「：」) under it. */
export function ErrorNotice({ text, onClose }: { text: string; onClose: () => void }) {
  const at = text.indexOf("：");
  const [title, desc] = at > 0 && at <= 16 ? [text.slice(0, at), text.slice(at + 1)] : [text, undefined];
  return (
    <NoticeCard
      tone="error"
      testId="connect-error"
      icon={<CircleAlert size={18} strokeWidth={1.7} />}
      title={title}
      desc={desc || undefined}
      onClose={onClose}
    />
  );
}
