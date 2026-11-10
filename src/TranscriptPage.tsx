import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { TranscriptView } from "@/features/transcript";
import { api } from "@/ipc";
import type { RemoteApi } from "@/ipc/remote";
import { useUi } from "@/state/ui";

/** The iPhone app's read-only Transcript (WKWebView): only the conversation; Connect and the composer are native. */
export default function TranscriptPage() {
  const qc = useQueryClient();

  useEffect(() => {
    useUi.getState().selectSession(new URLSearchParams(window.location.search).get("sessionId"));
    window.webkit?.messageHandlers?.codeck?.postMessage({ type: "ready" });
  }, []);

  // The event socket does not replay: whatever is shown is refetched after every (re)connect.
  useEffect(() => (api as Partial<RemoteApi>).onResync?.(() => void qc.invalidateQueries()), [qc]);

  // Tells the app when the conversation leaves its end. Two thresholds: the row this collapses changes the scroller's
  // height, which must not flip the answer back.
  useEffect(() => {
    let atBottom = true;
    const onScroll = (e: Event) => {
      const el = e.target;
      if (!(el instanceof HTMLElement) || !el.hasAttribute("data-scope")) return;
      const gap = el.scrollHeight - el.scrollTop - el.clientHeight;
      const next = gap > 160 ? false : gap < 60 ? true : atBottom;
      if (next === atBottom) return;
      atBottom = next;
      window.webkit?.messageHandlers?.codeck?.postMessage({ type: "scroll", atBottom });
    };
    document.addEventListener("scroll", onScroll, { capture: true, passive: true });
    return () => document.removeEventListener("scroll", onScroll, { capture: true });
  }, []);

  return (
    <div data-testid="transcript-page" className="transcript-page flex h-full min-h-0 flex-col bg-ground">
      <TranscriptView />
    </div>
  );
}
