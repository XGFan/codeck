import { useUi } from "@/state/ui";
import { ConnectPanel, ConnectRoot, ElsewhereBanner } from "@/features/connect";
import { FindBar } from "@/features/find";
import { StatsToggle, Toolbar, useSessionsHidden } from "@/features/layout";
import { Outline } from "@/features/outline";
import { SearchOverlay } from "@/features/search";
import { SessionHeader } from "@/features/header";
import { SessionList } from "@/features/sessions";
import { SettingsButton, SettingsDialog } from "@/features/settings";
import { StatsDialog } from "@/features/stats";
import { StatusBar } from "@/features/statusbar";
import { SubagentPanel } from "@/features/subagent-panel";
import { TranscriptView } from "@/features/transcript";
import { cn } from "@/lib/cn";

export default function App() {
  const sessionId = useUi((s) => s.sessionId);
  const hideList = useSessionsHidden().hidden;
  // The conversation column's width, for the transcript and the input area under it.
  const convWidth = useUi((s) => s.convWidth);

  return (
    <div className="grid h-full grid-rows-[1fr]" style={{ "--conv-w": `${convWidth}px` } as React.CSSProperties}>
      <div
        className="grid min-h-0"
        style={{ gridTemplateColumns: `${hideList ? 0 : 320}px 1fr` }}
      >
        <section
          data-testid="pane-sessions"
          aria-hidden={hideList || undefined}
          className={cn("flex min-h-0 flex-col border-r border-border bg-list", hideList && "invisible overflow-hidden border-r-0")}
        >
          <SessionList />
          {!hideList && <StatusBar />}
        </section>

        <main data-testid="pane-conversation" className="flex min-h-0 min-w-0 flex-col bg-ground">
          {!sessionId && <div data-tauri-drag-region="deep" className="h-[52px] shrink-0" />}
          <SessionHeader />
          {/* Under the header: the top strip is the title bar, where the toolbar floats once the list is hidden. */}
          <ElsewhereBanner />
          <FindBar />
          <div className="flex min-h-0 flex-1">
            {/* The input area floats over the foot of the transcript, as wide as it; the outline over its left margin. */}
            <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
              <TranscriptView />
              <ConnectPanel />
              <Outline />
            </div>
            <SubagentPanel />
          </div>
        </main>
      </div>
      <Toolbar>
        {/* With the list hidden, its foot's buttons join the toolbar. */}
        {hideList && (
          <>
            <StatsToggle />
            <SettingsButton className="size-7 text-secondary" />
          </>
        )}
      </Toolbar>
      <ConnectRoot />
      {/* Overlays: portal/fixed based; wrapped so placeholder stubs stay out of the layout. */}
      <div className="fixed right-0 bottom-0 size-0 overflow-hidden">
        <SearchOverlay />
        <StatsDialog />
        <SettingsDialog />
      </div>
    </div>
  );
}
