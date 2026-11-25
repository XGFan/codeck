import { create } from "zustand";
import { api, type Unlisten } from "@/ipc";
import { errorText } from "@/lib/errors";
import type { HaaWorker } from "@/ipc/protocol/haa";

export interface HaaWatchDeps {
  watch: (on: boolean, id: number) => Promise<void>;
  subscribeWorkers: (cb: (id: number, workers: HaaWorker[]) => void) => Promise<Unlisten>;
  subscribeEnded: (cb: (id: number, message: string) => void) => Promise<Unlisten>;
  onWorkers: (workers: HaaWorker[]) => void;
  onUnavailable: (reason: string) => void;
}

/**
 * Coordinates the desktop `haa ls --all --json --watch` process: waits for both subscriptions,
 * sweeps any watch left across reloads, serializes `watch` calls with an increasing id, and stops retrying on failure.
 */
export function createHaaWatch(o: HaaWatchDeps) {
  let seq = 0;
  let mount = 0;
  let ready = false;
  let wanted = false;
  let unavailable = false;
  let current: number | null = null;
  let hostRunning = false;
  let needsInitialStop = true;
  let busy = false;
  let again = false;

  const shouldRun = () => !unavailable && ready && wanted;

  const fail = (reason: string) => {
    if (unavailable) return;
    unavailable = true;
    current = null;
    o.onWorkers([]);
    o.onUnavailable(reason);
  };

  const invalidate = () => {
    if (current === null) return;
    current = null;
    o.onWorkers([]);
  };

  const reconcile = async () => {
    while (true) {
      if (needsInitialStop) {
        if (!ready) return;
        needsInitialStop = false;
        await o.watch(false, 0).catch(() => {});
      } else if (hostRunning && (!shouldRun() || current === null)) {
        current = null;
        hostRunning = false;
        o.onWorkers([]);
        await o.watch(false, 0).catch(() => {});
      } else if (shouldRun() && !hostRunning) {
        const id = ++seq;
        current = id;
        try {
          await o.watch(true, id);
        } catch (err) {
          fail(errorText(err) || "启动 haa 失败");
          return;
        }
        if (unavailable) return;
        hostRunning = true;
      } else {
        return;
      }
    }
  };

  const schedule = () => {
    again = true;
    if (busy) return;
    busy = true;
    void Promise.resolve().then(async () => {
      while (again) {
        again = false;
        await reconcile();
      }
      busy = false;
    });
  };

  return {
    start() {
      const my = ++mount;
      ready = false;
      const offs: Unlisten[] = [];
      const alive = () => mount === my;
      const stop = () => {
        if (!alive()) return false;
        mount++;
        ready = false;
        offs.splice(0).forEach((un) => un());
        invalidate();
        return true;
      };
      const keep = (p: Promise<Unlisten>) => p.then((un) => (alive() ? offs.push(un) : un()));
      void Promise.all([
        keep(o.subscribeWorkers((id, list) => alive() && id === current && o.onWorkers(list))),
        keep(
          o.subscribeEnded((id, msg) => {
            if (!alive() || id !== current) return;
            hostRunning = false;
            fail(msg || "haa ls 意外退出");
          }),
        ),
      ]).then(
        () => {
          if (!alive()) return;
          ready = true;
          schedule();
        },
        (err) => {
          if (stop()) fail(errorText(err) || "订阅 haa 事件失败");
        },
      );
      return () => {
        if (stop()) schedule();
      };
    },
    setWanted(w: boolean) {
      wanted = w;
      if (!shouldRun()) invalidate();
      schedule();
    },
  };
}

export interface HaaStore {
  /** Latest worker snapshot across all Sessions from `haa ls --all --json --watch`. */
  workers: HaaWorker[];
  start: () => () => void;
  setWanted: (wanted: boolean) => void;
}

export const useHaa = create<HaaStore>((set) => {
  const watch = createHaaWatch({
    watch: (on, id) => api.haaWatch(on, id),
    subscribeWorkers: (cb) => api.onHaaWorkers(cb),
    subscribeEnded: (cb) => api.onHaaWatchEnded(cb),
    onWorkers: (workers) => set((s) => (workers.length === 0 && s.workers.length === 0 ? s : { workers })),
    onUnavailable: (reason) => {
      console.warn(`haa Worker 状态不可用：${reason}`);
      set({ workers: [] });
    },
  });
  return {
    workers: [],
    start: watch.start,
    setWanted: watch.setWanted,
  };
});
