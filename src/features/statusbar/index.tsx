import { StatsToggle } from "@/features/layout";
import { SettingsButton } from "@/features/settings";
import { formatCount } from "@/lib/format";
import { useIndexStatus } from "@/queries";

/**
 * The foot of the Session list: index status, then stats and settings. While the list is hidden the two buttons
 * sit in the toolbar instead ([`Toolbar`]), so the conversation keeps no bar of its own.
 */
export function StatusBar() {
  const { data: s } = useIndexStatus();
  // Keyed so the buttons stay the same elements when the status first arrives: rebuilt, a click pressed just
  // before would be released on a new button and lost.
  const actions = (
    <span key="actions" className="ml-auto flex shrink-0 items-center gap-0.5">
      <StatsToggle className="size-6" />
      <SettingsButton className="size-6 text-secondary" />
    </span>
  );
  if (!s) return <div className="flex h-9 shrink-0 items-center border-t border-border px-1.5">{actions}</div>;

  const busy = s.phase === "scanning" || s.phase === "indexingText";
  const pct =
    s.bytesTotal > 0
      ? Math.min(100, (s.bytesDone / s.bytesTotal) * 100)
      : s.filesTotal > 0
        ? Math.min(100, (s.filesDone / s.filesTotal) * 100)
        : 0;
  return (
    <div
      data-testid="statusbar"
      data-tauri-drag-region="deep"
      className="flex h-9 shrink-0 items-center gap-3 border-t border-border pr-1.5 pl-4 text-[11px] text-secondary"
    >
      {s.phase === "error" ? (
        <span className="text-error">索引失败{s.error ? ` · ${s.error}` : ""}</span>
      ) : busy ? (
        <>
          <span>{s.phase === "scanning" ? "正在扫描" : "正在建立全文索引"}</span>
          <span className="tabular-nums">
            {formatCount(s.filesDone)} / {formatCount(s.filesTotal)} 个文件
          </span>
          <span
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(pct)}
            className="h-1 min-w-8 flex-1 overflow-hidden rounded-full bg-border"
          >
            <span className="block h-full rounded-full bg-accent transition-[width]" style={{ width: `${pct}%` }} />
          </span>
        </>
      ) : (
        <span>已索引 {formatCount(s.sessionsTotal)} 个 Session</span>
      )}
      {actions}
    </div>
  );
}
