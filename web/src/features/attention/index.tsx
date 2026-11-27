import { useEffect } from "react";
import { api } from "@/ipc";
import type { Unlisten } from "@/ipc/api";
import { useUi } from "@/state/ui";
import { LEGACY_KEY, legacyDelays } from "./migrate";
import { useAttentionStore } from "./store";

export { isKeeping, useAttentionStore } from "./store";

/** The notifications' settings exist only where the app runs on the Mac (not the LAN page). */
export const notifyAvailable = import.meta.env.VITE_IPC !== "remote";

/** The Session's turn ended while nobody looked, and it has not been looked at since (「已完成」, the Host's). */
export function useDone(sessionId: string): boolean {
  return useAttentionStore((s) => s.done.includes(sessionId));
}

/** The Session being looked at: the window is in front and shows it. */
function viewing(): string | null {
  if (document.visibilityState !== "visible" || !document.hasFocus()) return null;
  return useUi.getState().sessionId;
}

/** Delays an earlier version kept in localStorage go to the Host's settings, once. */
async function migrateLegacyDelays() {
  const legacy = legacyDelays(localStorage.getItem(LEGACY_KEY));
  if (legacy == null) return;
  const { settings } = await api.notifySettings();
  await api.setNotifySettings({ ...settings, ...legacy });
  localStorage.removeItem(LEGACY_KEY);
}

/**
 * The Host decides the notifications and 「已完成」 (ADR-0019): this tells it which Session the window looks at, and
 * keeps 「已完成」 and the cache refreshes it reports. Mount once (`ConnectRoot`).
 */
export function useAttention() {
  useEffect(() => {
    let last: string | null | undefined;
    const report = () => {
      const v = viewing();
      if (v === last) return;
      last = v;
      void api.setViewing(v).catch(() => {});
    };
    report();
    window.addEventListener("focus", report);
    window.addEventListener("blur", report);
    document.addEventListener("visibilitychange", report);
    const unUi = useUi.subscribe((s, p) => {
      if (s.sessionId !== p.sessionId) report();
    });

    const store = useAttentionStore.getState();
    let gone = false;
    let reported = false;
    const unlisten: Unlisten[] = [];
    const keep = (p: Promise<Unlisten>) => void p.then((f) => (gone ? f() : unlisten.push(f)));
    keep(
      api.onAttention((done) => {
        reported = true;
        store.setDone(done);
      }),
    );
    // A reloaded window: what is done already (unless an event said otherwise meanwhile).
    void api
      .attentionDone()
      .then((done) => !gone && !reported && store.setDone(done))
      .catch(() => {});
    keep(api.onKeepAlive((e) => store.setKeeping(e.sessionId, e.active)));
    if (notifyAvailable) void migrateLegacyDelays().catch(() => {});
    return () => {
      gone = true;
      window.removeEventListener("focus", report);
      window.removeEventListener("blur", report);
      document.removeEventListener("visibilitychange", report);
      unUi();
      unlisten.forEach((f) => f());
    };
  }, []);
}
