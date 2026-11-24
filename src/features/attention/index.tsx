import { useEffect, useMemo, useRef } from "react";
import { api } from "@/ipc";
import { isLive, useConnect, waitingSessions } from "@/features/connect/store";
import { useAllSessions, useProjects } from "@/queries";
import { useUi } from "@/state/ui";
import { due, markNotified, message, step, type Status } from "./rules";
import { useAttentionStore } from "./store";

export { useAttentionStore } from "./store";

/** The notifications exist only where the app runs on the Mac (not the LAN page). */
export const notifyAvailable = import.meta.env.VITE_IPC !== "remote";

/** The Session's turn ended while nobody looked, and it has not been looked at since (「已完成」). */
export function useDone(sessionId: string): boolean {
  return useAttentionStore((s) => !!s.tracks[sessionId]?.done);
}

/** The Session being looked at: the window is in front and shows it. */
function viewing(): string | null {
  if (document.visibilityState !== "visible" || !document.hasFocus()) return null;
  return useUi.getState().sessionId;
}

/**
 * Keeps 「已完成」 and sends the notifications, from the Live Sessions' statuses: a connected one's from ccrpc
 * (it knows at once), the others' from their process files, waiting when Live waits or a request card is open.
 * Mount once (`ConnectRoot`).
 */
export function useAttention(live: { id: string; status: Status }[]) {
  const conns = useConnect((s) => s.conns);
  const statuses = useMemo(() => {
    const out: Record<string, Status> = {};
    for (const l of live) out[l.id] = l.status;
    for (const c of Object.values(conns)) {
      const id = c.sessionId;
      if (id == null || !(id in out) || !isLive(c)) continue;
      if (c.state === "busy" || c.state === "idle" || c.state === "waiting") out[id] = c.state;
    }
    const waiting = live.filter((l) => l.status === "waiting").map((l) => l.id);
    for (const id of waitingSessions(waiting, conns)) if (id in out) out[id] = "waiting";
    return out;
  }, [live, conns]);
  const { data: sessions } = useAllSessions();
  const { data: projects } = useProjects();
  const latest = useRef({ statuses, sessions, projects });
  latest.current = { statuses, sessions, projects };

  const update = useRef(() => {
    const { statuses, sessions, projects } = latest.current;
    const store = useAttentionStore.getState();
    const at = Date.now();
    let tracks = step(store.tracks, statuses, at, viewing());
    const fire = notifyAvailable ? due(tracks, at, store.delays) : [];
    if (fire.length > 0) {
      tracks = markNotified(
        tracks,
        fire.map((f) => f.id),
      );
      const ls = useConnect.getState().ls;
      const items = fire.map((f) => {
        const s = sessions?.find((x) => x.id === f.id);
        const entry = ls.find((e) => e.session_id === f.id);
        return {
          ...f,
          title: s?.title ?? entry?.title ?? "新 Session",
          project: projects?.find((p) => p.id === s?.projectId)?.displayName ?? entry?.cwd?.split("/").pop() ?? null,
        };
      });
      const m = message(items, at);
      void api.notify({ title: m.title, body: m.body, url: `codeck://session/${m.sessionId}` }).catch(() => {});
    }
    if (tracks !== store.tracks) store.setTracks(tracks);
  }).current;

  useEffect(update, [statuses, update]);
  useEffect(() => {
    // Wall-clock based: a timer the system slowed down only sends late, never wrong.
    const timer = window.setInterval(update, 1000);
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    document.addEventListener("visibilitychange", update);
    const unUi = useUi.subscribe((s, p) => {
      if (s.sessionId !== p.sessionId) update();
    });
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
      document.removeEventListener("visibilitychange", update);
      unUi();
    };
  }, [update]);
}
