import { useEffect } from "react";
import { api } from "@/ipc";
import { parseDeepLink } from "@/lib/deepLink";
import { useUi } from "@/state/ui";

/** Opens the Session a `codeck://` link names (the window is brought back by the shell); others are ignored. */
export function useDeepLinks() {
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void api
      .onOpenUrl((urls) => {
        const link = urls.map(parseDeepLink).findLast((l) => l != null);
        if (link) openSession(link.sessionId);
      })
      .then((fn) => (disposed ? fn() : (unlisten = fn)));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}

/** Like picking it in ⌘K or in the stats: nothing hides it, whatever was open. */
function openSession(id: string) {
  const ui = useUi.getState();
  ui.setSearchOpen(false);
  ui.setStatsOpen(false);
  ui.setSettingsOpen(false);
  ui.setProjectIds([]);
  ui.setStatsDrill(null);
  ui.selectSession(id);
}
