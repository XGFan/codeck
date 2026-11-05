import { cn } from "@/lib/cn";

export type LiveDotState = "busy" | "waiting" | "idle";

/**
 * A Live Session's state as a dot: busy green, waiting for an answer orange, idle a green ring. `halo` adds a soft
 * ring around busy / waiting (the Session list).
 */
export function LiveDot({
  state,
  halo = false,
  title,
  testId,
}: {
  state: LiveDotState;
  halo?: boolean;
  title?: string;
  testId?: string;
}) {
  return (
    <span
      data-testid={testId}
      data-state={state}
      title={title}
      className={cn(
        "size-2 shrink-0 rounded-full",
        state === "busy" && "bg-live",
        state === "waiting" && "bg-live-waiting",
        state === "idle" && "box-border border-[1.5px] border-live",
        halo && state === "busy" && "shadow-[0_0_0_3px_rgba(30,127,69,0.16)]",
        halo &&
          state === "waiting" &&
          "shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-live-waiting)_20%,transparent)]",
      )}
    />
  );
}
