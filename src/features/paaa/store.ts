import { create } from "zustand";
import { api, type Unlisten } from "@/ipc";
import { errorText } from "@/lib/errors";
import type { PaaaWorker } from "./protocol";

export interface PaaaWatchDeps {
  watch: (on: boolean, id: number) => Promise<void>;
  subscribeWorkers: (cb: (id: number, workers: PaaaWorker[]) => void) => Promise<Unlisten>;
  subscribeEnded: (cb: (id: number, message: string) => void) => Promise<Unlisten>;
  onWorkers: (workers: PaaaWorker[]) => void;
  onUnavailable: (reason: string) => void;
}

/**
 * Coordinates the desktop `paaa ls --all --json --watch` process: waits for both subscriptions,
 * sweeps any watch left across reloads, serializes `watch` calls with an increasing id, and stops retrying on failure.
 */
export function createPaaaWatch(o: PaaaWatchDeps) {
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
          fail(errorText(err) || "启动 paaa 失败");
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
            fail(msg || "paaa ls 意外退出");
          }),
        ),
      ]).then(
        () => {
          if (!alive()) return;
          ready = true;
          schedule();
        },
        (err) => {
          if (stop()) fail(errorText(err) || "订阅 paaa 事件失败");
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

export interface PaaaStore {
  /** Latest worker snapshot across all Sessions from `paaa ls --all --json --watch`. */
  workers: PaaaWorker[];
  start: () => () => void;
  setWanted: (wanted: boolean) => void;
}

export const usePaaa = create<PaaaStore>((set) => {
  const watch = createPaaaWatch({
    watch: (on, id) => api.paaaWatch(on, id),
    subscribeWorkers: (cb) => api.onPaaaWorkers(cb),
    subscribeEnded: (cb) => api.onPaaaWatchEnded(cb),
    onWorkers: (workers) => set((s) => (workers.length === 0 && s.workers.length === 0 ? s : { workers })),
    onUnavailable: (reason) => {
      console.warn(`paaa Worker 状态不可用：${reason}`);
      set({ workers: [] });
    },
  });
  return {
    workers: [],
    start: watch.start,
    setWanted: watch.setWanted,
  };
});
