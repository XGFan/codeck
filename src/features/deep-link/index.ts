import { useEffect } from "react";
import { api } from "@/ipc";
import { parseDeepLink } from "@/lib/deepLink";
import { useUi } from "@/state/ui";

/**
 * Opens the Session a `codeck://` link names, opened in the system or carried by a clicked notification (the window
 * is brought back by the shell); others are ignored.
 */
export function useDeepLinks() {
  useEffect(() => {
    let disposed = false;
    const unlisten: (() => void)[] = [];
    const open = (urls: string[]) => {
      const link = urls.map(parseDeepLink).findLast((l) => l != null);
      if (link) openSession(link.sessionId);
    };
    const keep = (p: Promise<() => void>) => void p.then((fn) => (disposed ? fn() : unlisten.push(fn)));
    keep(api.onOpenUrl(open));
    keep(api.onNotificationClick((url) => open([url])));
    return () => {
      disposed = true;
      unlisten.forEach((fn) => fn());
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
