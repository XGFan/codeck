import { create } from "zustand";
import type { Delays, DueKind, Tracks } from "./rules";

const KEY = "codeck.notify";
/** 3 minutes for both, unless set in 「设置 › 通用」. */
const DEFAULT_DELAYS: Delays = { waiting: 3 * 60_000, done: 3 * 60_000 };

function load(): Delays {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<Delays>;
    const pick = (v: unknown, d: number | null) => (v === null || (typeof v === "number" && v >= 0) ? v : d);
    return { waiting: pick(saved.waiting, DEFAULT_DELAYS.waiting), done: pick(saved.done, DEFAULT_DELAYS.done) };
  } catch {
    return DEFAULT_DELAYS;
  }
}

interface AttentionState {
  /** Each Live Session's status, since when, 「已完成」 and whether its notification went out (see `rules.ts`). */
  tracks: Tracks;
  delays: Delays;
  /** Sessions whose prompt cache the Host is refreshing (a `/btw` of its own, so Live says waiting), since when. */
  keeping: Record<string, number>;
  setKeeping(sessionId: string, active: boolean): void;
  setTracks(tracks: Tracks): void;
  /** `null` turns that notification off. */
  setDelay(kind: DueKind, ms: number | null): void;
}

/** A refresh takes seconds; one whose end never came (the Host went away) stops counting after this. */
const KEEPING_MAX_MS = 3 * 60_000;

/** The Host is refreshing this Session's cache now: its waiting is the `/btw` panel, not a question. */
export const isKeeping = (keeping: Record<string, number>, sessionId: string, now = Date.now()) =>
  keeping[sessionId] != null && now - keeping[sessionId] < KEEPING_MAX_MS;

export const useAttentionStore = create<AttentionState>((set, get) => ({
  tracks: {},
  delays: load(),
  keeping: {},
  setKeeping(sessionId, active) {
    const keeping = { ...get().keeping };
    if (active) keeping[sessionId] = Date.now();
    else delete keeping[sessionId];
    set({ keeping });
  },
  setTracks: (tracks) => set({ tracks }),
  setDelay(kind, ms) {
    const delays = { ...get().delays, [kind]: ms };
    try {
      localStorage.setItem(KEY, JSON.stringify(delays));
    } catch {
      // Not saved: it still holds until the app restarts.
    }
    set({ delays });
  },
}));
