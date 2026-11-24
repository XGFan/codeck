import type { PaaaWorker } from "@/features/paaa/protocol";
import type { Api, Unlisten } from "./api";

export interface PaaaProbe {
  /** A `paaa ls --all --json --watch` is running. */
  watching: boolean;
  /** `paaaWatch(true, id)` calls. */
  watchStarts: number;
  /** True once both event subscriptions are active and the startup sweep has run. */
  ready: boolean;
  /** Pushes a worker snapshot tagged with the currently active watch id. */
  emit: (workers: PaaaWorker[]) => void;
  /** Pushes a watch-ended event tagged with the currently active watch id. */
  end: (message: string) => void;
}

declare global {
  interface Window {
    /** E2E spy on the mock paaa watch. */
    __cvPaaa?: PaaaProbe;
  }
}

type PaaaApi = Pick<Api, "paaaWatch" | "onPaaaWorkers" | "onPaaaWatchEnded">;

export function createMockPaaa(): PaaaApi {
  const workerCbs = new Set<(id: number, workers: PaaaWorker[]) => void>();
  const endedCbs = new Set<(id: number, message: string) => void>();
  let currentId: number | null = null;
  let swept = false;

  const updateReady = () => {
    probe.ready = workerCbs.size > 0 && endedCbs.size > 0 && swept;
  };

  const probe: PaaaProbe = {
    watching: false,
    watchStarts: 0,
    ready: false,
    emit(workers) {
      if (currentId == null) return;
      for (const cb of workerCbs) cb(currentId, workers);
    },
    end(message) {
      if (currentId == null) return;
      const id = currentId;
      probe.watching = false;
      currentId = null;
      for (const cb of endedCbs) cb(id, message);
    },
  };

  if (typeof window !== "undefined") window.__cvPaaa = probe;

  const subscribe = <T>(set: Set<T>, cb: T): Promise<Unlisten> => {
    set.add(cb);
    updateReady();
    return Promise.resolve(() => {
      set.delete(cb);
      updateReady();
    });
  };

  return {
    paaaWatch: async (on, id) => {
      if (on) {
        probe.watching = true;
        probe.watchStarts++;
        currentId = id;
      } else {
        probe.watching = false;
        swept = true;
        currentId = null;
      }
      updateReady();
    },
    onPaaaWorkers: (cb) => subscribe(workerCbs, cb),
    onPaaaWatchEnded: (cb) => subscribe(endedCbs, cb),
  };
}
