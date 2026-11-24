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
  setTracks(tracks: Tracks): void;
  /** `null` turns that notification off. */
  setDelay(kind: DueKind, ms: number | null): void;
}

export const useAttentionStore = create<AttentionState>((set, get) => ({
  tracks: {},
  delays: load(),
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
