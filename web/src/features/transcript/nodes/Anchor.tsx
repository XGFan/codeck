import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { anchorAttr, useHighlighted } from "./scroll";

/** Scroll target for a node / tool call; shows the flash ring while `useUi.highlight` points at it. */
export function Anchor({
  nodeId,
  toolUseId,
  className,
  children,
}: {
  nodeId: string;
  toolUseId?: string | null;
  className?: string;
  children: ReactNode;
}) {
  const on = useHighlighted(nodeId, toolUseId);
  return (
    <div
      {...anchorAttr(nodeId, toolUseId)}
      data-highlighted={on || undefined}
      className={cn(
        "rounded-lg transition-shadow duration-500",
        on && "shadow-[0_0_0_2px_var(--color-ground),0_0_0_4px_var(--color-accent)]",
        className,
      )}
    >
      {children}
    </div>
  );
}
