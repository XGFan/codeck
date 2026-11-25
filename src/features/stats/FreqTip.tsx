import { useEffect, useLayoutEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { formatCount } from "@/lib/format";
import type { Theme } from "./charts";

export interface FreqTipTarget {
  anchor: HTMLElement;
  title: string;
  requests: number;
}

const EDGE_PAD = 8;
const ANCHOR_GAP = 8;

/**
 * Instant hover tooltip shared by the hourly grid and month-block grid in 「使用频率」.
 * Styled to match ECharts' `base(t).tooltip` and clamped inside the enclosing dialog & viewport.
 */
export function FreqTip({ tip, theme, onDismiss }: { tip: FreqTipTarget | null; theme: Theme; onDismiss: () => void }) {
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !tip) return;
    const rect = tip.anchor.getBoundingClientRect();
    const dialog = tip.anchor.closest('[role="dialog"]');
    const dRect = dialog?.getBoundingClientRect();

    const minX = Math.max(EDGE_PAD, (dRect?.left ?? 0) + EDGE_PAD);
    const maxX = Math.min(window.innerWidth - EDGE_PAD, (dRect?.right ?? window.innerWidth) - EDGE_PAD);
    const minY = Math.max(EDGE_PAD, (dRect?.top ?? 0) + EDGE_PAD);
    const maxY = Math.min(window.innerHeight - EDGE_PAD, (dRect?.bottom ?? window.innerHeight) - EDGE_PAD);

    const tipW = el.offsetWidth;
    const tipH = el.offsetHeight;

    let left = rect.left + rect.width / 2 - tipW / 2;
    left = Math.max(minX, Math.min(left, maxX - tipW));

    let top = rect.top - tipH - ANCHOR_GAP;
    if (top < minY) top = rect.bottom + ANCHOR_GAP;
    top = Math.max(minY, Math.min(top, maxY - tipH));

    el.style.left = `${Math.round(left)}px`;
    el.style.top = `${Math.round(top)}px`;
    el.style.visibility = "visible";
  }, [tip]);

  useEffect(() => {
    if (!tip) return;
    const dismiss = () => onDismiss();
    window.addEventListener("scroll", dismiss, { capture: true, passive: true });
    return () => window.removeEventListener("scroll", dismiss, { capture: true });
  }, [tip, onDismiss]);

  if (!tip) return null;

  return createPortal(
    <div
      ref={ref}
      data-testid="freq-tip"
      role="tooltip"
      style={{
        backgroundColor: theme.tooltipBg,
        borderColor: theme.grid,
        color: theme.ink,
        boxShadow: "0 4px 14px rgba(0,0,0,.18)",
        visibility: "hidden",
      }}
      className="pointer-events-none fixed top-0 left-0 z-60 rounded-[6px] border px-[9px] py-[6px] text-[12px] leading-[1.5] whitespace-nowrap"
    >
      <div className="font-semibold">{tip.title}</div>
      <div className="tabular-nums">{formatCount(tip.requests)} 次请求</div>
    </div>,
    document.body,
  );
}
