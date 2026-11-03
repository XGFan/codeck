import { Check, Copy } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "@/ipc";
import { cn } from "@/lib/cn";

/** Small "复制" action; shows "已复制" for a moment. Callers position it and reveal it on hover. */
export function CopyButton({ text, label = "复制", className }: { text: string; label?: string; className?: string }) {
  const [done, setDone] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return (
    <button
      type="button"
      data-testid="copy-button"
      aria-label={label}
      onClick={(e) => {
        e.stopPropagation();
        void api.copyText(text);
        setDone(true);
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setDone(false), 1200);
      }}
      className={cn(
        "flex h-5 items-center gap-1 rounded px-1.5 text-[11px] text-secondary opacity-0 transition-opacity group-focus-within/copy:opacity-100 group-hover/copy:opacity-100 hover:bg-selection hover:text-text focus-visible:opacity-100",
        done && "opacity-100",
        className,
      )}
    >
      {done ? <Check size={11} strokeWidth={2} aria-hidden /> : <Copy size={11} strokeWidth={1.6} aria-hidden />}
      {done ? "已复制" : label}
    </button>
  );
}
