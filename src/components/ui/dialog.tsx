import { Dialog as DialogPrimitive } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "@/lib/cn";

export const Dialog = DialogPrimitive.Root;

export function DialogContent({ className, children, ...props }: ComponentProps<typeof DialogPrimitive.Content>) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-black/40" />
      <DialogPrimitive.Content
        className={cn(
          "fixed left-1/2 top-1/2 z-50 w-[480px] max-w-[90vw] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-ground p-4 shadow-xl",
          className,
        )}
        {...props}
      >
        {children}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

/**
 * A large panel over the window (⌘K search, stats): a lighter backdrop, rounded 14px, a deeper shadow. Centered
 * horizontally; the caller places it vertically and sizes it.
 */
export function PanelDialogContent({
  className,
  overlayTestId,
  children,
  ...props
}: ComponentProps<typeof DialogPrimitive.Content> & { overlayTestId?: string }) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay data-testid={overlayTestId} className="fixed inset-0 z-50 bg-[rgba(28,28,30,0.28)]" />
      <DialogPrimitive.Content
        className={cn(
          "fixed left-1/2 z-50 -translate-x-1/2 overflow-hidden rounded-[14px] border border-border text-text shadow-[0_24px_64px_rgba(0,0,0,0.22),0_2px_8px_rgba(0,0,0,0.08)]",
          className,
        )}
        {...props}
      >
        {children}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

export function DialogTitle({ className, ...props }: ComponentProps<typeof DialogPrimitive.Title>) {
  return <DialogPrimitive.Title className={cn("mb-3 text-[15px] font-semibold", className)} {...props} />;
}

export const DialogDescription = DialogPrimitive.Description;
