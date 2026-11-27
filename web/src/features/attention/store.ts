import { create } from "zustand";

interface AttentionState {
  /** The 「已完成」 Sessions, as the Host reports them (it decides, ADR-0019). */
  done: string[];
  setDone(done: string[]): void;
  /** Sessions whose prompt cache the Host is refreshing (a `/btw` of its own, so Live says waiting), since when. */
  keeping: Record<string, number>;
  setKeeping(sessionId: string, active: boolean): void;
}

/** A refresh takes seconds; one whose end never came (the Host went away) stops counting after this. */
const KEEPING_MAX_MS = 3 * 60_000;

/** The Host is refreshing this Session's cache now: its waiting is the `/btw` panel, not a question. */
export const isKeeping = (keeping: Record<string, number>, sessionId: string, now = Date.now()) =>
  keeping[sessionId] != null && now - keeping[sessionId] < KEEPING_MAX_MS;

export const useAttentionStore = create<AttentionState>((set, get) => ({
  done: [],
  setDone: (done) => set({ done }),
  keeping: {},
  setKeeping(sessionId, active) {
    const keeping = { ...get().keeping };
    if (active) keeping[sessionId] = Date.now();
    else delete keeping[sessionId];
    set({ keeping });
  },
}));
