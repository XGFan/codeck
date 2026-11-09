import { StatsToggle } from "@/features/layout";
import { SettingsButton } from "@/features/settings";
import { useIndexStatus } from "@/queries";

/**
 * The foot of the Session list: settings and stats in the window's corner, then an index failure if there is one (the
 * count and the progress are in Settings: here they changed as Sessions were written, while typing). While the list is
 * hidden the two buttons sit in the toolbar instead ([`Toolbar`]), so the conversation keeps no bar of its own.
 */
export function StatusBar() {
  const { data: s } = useIndexStatus();
  // Keyed so the buttons stay the same elements when the status first arrives: rebuilt, a click pressed just
  // before would be released on a new button and lost.
  const actions = (
    <span key="actions" className="flex shrink-0 items-center gap-0.5">
      <SettingsButton className="size-6 text-secondary" />
      <StatsToggle className="size-6" />
    </span>
  );
  if (!s) return <div className="flex h-9 shrink-0 items-center border-t border-border px-1.5">{actions}</div>;

  return (
    <div
      data-testid="statusbar"
      data-tauri-drag-region="deep"
      className="flex h-9 shrink-0 items-center gap-3 border-t border-border px-1.5 text-[11px] text-secondary"
    >
      {actions}
      {s.phase === "error" && <span className="text-error">索引失败{s.error ? ` · ${s.error}` : ""}</span>}
    </div>
  );
}
