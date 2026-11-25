import type { HaaWorker } from "@/features/haa/protocol";
import type { Api, Unlisten } from "@/ipc/api";

export interface HaaProbe {
  /** A `haa ls --all --json --watch` is running. */
  watching: boolean;
  /** `haaWatch(true, id)` calls. */
  watchStarts: number;
  /** True once both event subscriptions are active and the startup sweep has run. */
  ready: boolean;
  /** Pushes a worker snapshot tagged with the currently active watch id. */
  emit: (workers: HaaWorker[]) => void;
  /** Pushes a watch-ended event tagged with the currently active watch id. */
  end: (message: string) => void;
}

declare global {
  interface Window {
    /** E2E spy on the mock haa watch. */
    __cvHaa?: HaaProbe;
  }
}

type HaaApi = Pick<Api, "haaWatch" | "onHaaWorkers" | "onHaaWatchEnded">;

export function createMockHaa(): HaaApi {
  const workerCbs = new Set<(id: number, workers: HaaWorker[]) => void>();
  const endedCbs = new Set<(id: number, message: string) => void>();
  let currentId: number | null = null;
  let swept = false;

  const updateReady = () => {
    probe.ready = workerCbs.size > 0 && endedCbs.size > 0 && swept;
  };

  const probe: HaaProbe = {
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

  if (typeof window !== "undefined") window.__cvHaa = probe;

  const subscribe = <T>(set: Set<T>, cb: T): Promise<Unlisten> => {
    set.add(cb);
    updateReady();
    return Promise.resolve(() => {
      set.delete(cb);
      updateReady();
    });
  };

  return {
    haaWatch: async (on, id) => {
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
    onHaaWorkers: (cb) => subscribe(workerCbs, cb),
    onHaaWatchEnded: (cb) => subscribe(endedCbs, cb),
  };
}
