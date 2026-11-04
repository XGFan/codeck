import { useUi } from "@/state/ui";
import { DrivePanel, DriveRoot } from "@/features/drive";
import { FindBar } from "@/features/find";
import { Toolbar, useSessionsHidden } from "@/features/layout";
import { Outline } from "@/features/outline";
import { SearchOverlay } from "@/features/search";
import { SessionHeader } from "@/features/header";
import { SessionList } from "@/features/sessions";
import { SettingsDialog } from "@/features/settings";
import { StatsDialog } from "@/features/stats";
import { StatusBar } from "@/features/statusbar";
import { SubagentPanel } from "@/features/subagent-panel";
import { TranscriptView } from "@/features/transcript";
import { cn } from "@/lib/cn";

export default function App() {
  const sessionId = useUi((s) => s.sessionId);
  // Besides the user's toggle, the Subagent panel takes the session list's width so the conversation keeps
  // a readable column.
  const hideList = useSessionsHidden().hidden;

  return (
    <div className="grid h-full grid-rows-[1fr_28px]">
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
        </section>

        <main data-testid="pane-conversation" className="flex min-h-0 min-w-0 flex-col bg-ground">
          {!sessionId && <div data-tauri-drag-region="deep" className="h-[52px] shrink-0" />}
          <SessionHeader />
          <FindBar />
          <div className="flex min-h-0 flex-1">
            {/* The drive panel sits under the transcript, as wide as it. */}
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">
              <TranscriptView />
              <DrivePanel />
            </div>
            <Outline />
            <SubagentPanel />
          </div>
        </main>
      </div>
      <StatusBar />
      <Toolbar />
      <DriveRoot />
      {/* Overlays: portal/fixed based; wrapped so placeholder stubs stay out of the layout. */}
      <div className="fixed right-0 bottom-0 size-0 overflow-hidden">
        <SearchOverlay />
        <StatsDialog />
        <SettingsDialog />
      </div>
    </div>
  );
}
