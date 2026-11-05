import { useRef } from "react";
import { cn } from "@/lib/cn";
import { CONV_WIDTH, useUi } from "@/state/ui";

/**
 * The conversation column's edges, to drag its width (both sides move, it stays centered); double-click goes
 * back to the default; ← / → when focused. Lies over the column as it is laid out (narrower when the pane is).
 */
export function WidthHandles() {
  const box = useRef<HTMLDivElement>(null);
  const setWidth = useUi((s) => s.setConvWidth);
  const drag = (side: -1 | 1) => (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !box.current) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const x0 = e.clientX;
    const w0 = box.current.getBoundingClientRect().width;
    const move = (m: PointerEvent) => setWidth(w0 + side * 2 * (m.clientX - x0));
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
      document.body.style.cursor = "";
    };
    document.body.style.cursor = "col-resize";
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
  };
  const keys = (side: -1 | 1) => (e: React.KeyboardEvent) => {
    const d = e.key === "ArrowRight" ? 20 : e.key === "ArrowLeft" ? -20 : 0;
    if (!d || !box.current) return;
    e.preventDefault();
    setWidth(box.current.getBoundingClientRect().width + side * 2 * d);
  };
  const handle = (side: -1 | 1) => (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="拖动调节对话宽度，双击恢复默认"
      title="拖动调节对话宽度，双击恢复默认"
      tabIndex={0}
      data-testid={side < 0 ? "width-handle-left" : "width-handle-right"}
      onPointerDown={drag(side)}
      onDoubleClick={() => setWidth(CONV_WIDTH.default)}
      onKeyDown={keys(side)}
      className={cn(
        "group/w pointer-events-auto absolute inset-y-0 w-3 cursor-col-resize outline-none",
        side < 0 ? "-left-4" : "-right-4",
      )}
    >
      <span className="absolute inset-y-0 left-1/2 w-px bg-transparent transition-colors group-hover/w:bg-border group-focus-visible/w:bg-accent" />
    </div>
  );
  return (
    <div
      ref={box}
      data-testid="conversation-column"
      className="pointer-events-none absolute inset-y-0 left-1/2 -translate-x-1/2"
      style={{ width: "min(var(--conv-w), calc(100% - 48px))" }}
    >
      {handle(-1)}
      {handle(1)}
    </div>
  );
}
