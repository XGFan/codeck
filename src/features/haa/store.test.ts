import { describe, expect, it, vi } from "vitest";
import type { HaaWorker } from "./protocol";
import { createHaaWatch } from "./store";

type Deferred<T = void> = { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void };

function deferred<T = void>(): Deferred<T> {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => ((resolve = res), (reject = rej)));
  return { promise, resolve, reject };
}

const flush = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

const worker = (name: string): HaaWorker => ({ id: `s1:${name}`, owner: "s1", name, state: "running", tokens: 100 });

function setup(opts?: { delaySubscribe?: boolean }) {
  let autoResolve = true;
  const calls: { on: boolean; id: number; d: Deferred }[] = [];
  const workerSubs: { cb: (id: number, list: HaaWorker[]) => void; unlisten: ReturnType<typeof vi.fn<() => void>> }[] =
    [];
  const endedSubs: { cb: (id: number, msg: string) => void; unlisten: ReturnType<typeof vi.fn<() => void>> }[] = [];
  const pendingSubs: Deferred<() => void>[] = [];
  const workersLog: HaaWorker[][] = [];
  const unavailableLog: string[] = [];

  const sub = <T>(list: { cb: T; unlisten: ReturnType<typeof vi.fn<() => void>> }[], cb: T) => {
    const unlisten = vi.fn<() => void>();
    list.push({ cb, unlisten });
    if (!opts?.delaySubscribe) return Promise.resolve(unlisten);
    const d = deferred<() => void>();
    pendingSubs.push(d);
    return d.promise;
  };

  const ctrl = createHaaWatch({
    watch: (on, id) => {
      const d = deferred();
      calls.push({ on, id, d });
      if (autoResolve) d.resolve();
      return d.promise;
    },
    subscribeWorkers: (cb) => sub(workerSubs, cb),
    subscribeEnded: (cb) => sub(endedSubs, cb),
    onWorkers: (list) => workersLog.push(list),
    onUnavailable: (reason) => unavailableLog.push(reason),
  });

  return {
    ctrl,
    calls,
    workerSubs,
    endedSubs,
    pendingSubs,
    workersLog,
    unavailableLog,
    setAutoResolve: (v: boolean) => (autoResolve = v),
    emitWorkers: (id: number, list: HaaWorker[]) =>
      workerSubs.forEach((s) => s.unlisten.mock.calls.length === 0 && s.cb(id, list)),
    emitEnded: (id: number, msg: string) =>
      endedSubs.forEach((s) => s.unlisten.mock.calls.length === 0 && s.cb(id, msg)),
  };
}

describe("createHaaWatch", () => {
  it("waits for both subscriptions, sweeps once (ignoring sweep failure), then starts", async () => {
    const t = setup({ delaySubscribe: true });
    t.setAutoResolve(false);
    const stop = t.ctrl.start();
    t.ctrl.setWanted(true);
    await flush();
    expect(t.calls).toHaveLength(0);

    t.pendingSubs[0]!.resolve(t.workerSubs[0]!.unlisten);
    await flush();
    expect(t.calls).toHaveLength(0);

    t.pendingSubs[1]!.resolve(t.endedSubs[0]!.unlisten);
    await flush();
    expect(t.calls.map((c) => [c.on, c.id])).toEqual([[false, 0]]);

    t.calls[0]!.d.reject(new Error("ipc busy"));
    await flush();
    expect(t.unavailableLog).toEqual([]);
    expect(t.calls.map((c) => c.on)).toEqual([false, true]);
    expect(t.calls[1]!.id).toBeGreaterThanOrEqual(1);
    t.calls[1]!.d.resolve();
    stop();
  });

  it("tags each start with an increasing id and ignores stale events", async () => {
    const t = setup();
    const stop = t.ctrl.start();
    t.ctrl.setWanted(true);
    await flush();
    const id1 = t.calls[1]!.id;
    t.ctrl.setWanted(false);
    await flush();
    t.ctrl.setWanted(true);
    await flush();
    const id2 = t.calls[3]!.id;
    expect(id2).toBeGreaterThan(id1);

    t.emitWorkers(id2, [worker("fresh")]);
    t.emitWorkers(id1, [worker("stale")]);
    t.emitEnded(id1, "old crash");
    expect(t.unavailableLog).toEqual([]);
    expect(t.workersLog.at(-1)).toEqual([worker("fresh")]);
    stop();
  });

  it("serializes watch calls and clears workers immediately when cancelled mid-start", async () => {
    const t = setup();
    const stop = t.ctrl.start();
    await flush();
    t.setAutoResolve(false);
    t.ctrl.setWanted(true);
    await flush();
    const id1 = t.calls[1]!.id;
    t.emitWorkers(id1, [worker("w1")]);

    t.ctrl.setWanted(false);
    expect(t.workersLog.at(-1)).toEqual([]);
    expect(t.calls.map((c) => c.on)).toEqual([false, true]);
    t.emitWorkers(id1, [worker("late")]);
    expect(t.workersLog.at(-1)).toEqual([]);

    t.calls[1]!.d.resolve();
    await flush();
    expect(t.calls.map((c) => c.on)).toEqual([false, true, false]);
    t.calls[2]!.d.resolve();
    stop();
  });

  it("marks unavailable and never retries after watch(true) rejects or current Ended arrives", async () => {
    for (const kind of ["reject", "ended"] as const) {
      const t = setup();
      const stop = t.ctrl.start();
      await flush();
      t.setAutoResolve(false);
      t.ctrl.setWanted(true);
      await flush();
      if (kind === "reject") {
        t.calls[1]!.d.reject({ message: "没有找到 haa" });
      } else {
        const id = t.calls[1]!.id;
        t.emitWorkers(id, [worker("w1")]);
        t.emitEnded(id, "旧版不支持");
        t.calls[1]!.d.resolve();
      }
      await flush();
      t.ctrl.setWanted(false);
      t.ctrl.setWanted(true);
      await flush();
      expect(t.unavailableLog).toEqual([kind === "reject" ? "没有找到 haa" : "旧版不支持"]);
      expect(t.workersLog.at(-1)).toEqual([]);
      expect(t.calls.map((c) => c.on)).toEqual([false, true]);
      stop();
    }
  });

  it("marks unavailable and releases listeners when a subscription fails", async () => {
    const t = setup({ delaySubscribe: true });
    const stop = t.ctrl.start();
    t.ctrl.setWanted(true);
    const un1 = vi.fn<() => void>();
    t.pendingSubs[0]!.resolve(un1);
    t.pendingSubs[1]!.reject(new Error("订阅失败"));
    await flush();
    expect(un1).toHaveBeenCalledTimes(1);
    expect(t.unavailableLog).toEqual(["订阅失败"]);
    expect(t.calls).toHaveLength(0);
    stop();
  });

  it("clears workers and stops on unwant and cleanup (ignoring stop failures)", async () => {
    const t = setup();
    const stop = t.ctrl.start();
    t.ctrl.setWanted(true);
    await flush();
    t.emitWorkers(t.calls[1]!.id, [worker("w1")]);

    t.setAutoResolve(false);
    t.ctrl.setWanted(false);
    expect(t.workersLog.at(-1)).toEqual([]);
    await flush();
    t.calls[2]!.d.reject(new Error("stop failed"));
    await flush();
    expect(t.unavailableLog).toEqual([]);

    t.setAutoResolve(true);
    t.ctrl.setWanted(true);
    await flush();
    t.emitWorkers(t.calls[3]!.id, [worker("w2")]);
    expect(t.workersLog.at(-1)).toEqual([worker("w2")]);

    stop();
    expect(t.workersLog.at(-1)).toEqual([]);
    await flush();
    expect(t.calls.map((c) => c.on)).toEqual([false, true, false, true, false]);
  });

  it("handles React StrictMode double-mount without duplicate starts or leaked listeners", async () => {
    const t = setup({ delaySubscribe: true });
    const clean1 = t.ctrl.start();
    t.ctrl.setWanted(true);
    clean1();
    const clean2 = t.ctrl.start();
    t.ctrl.setWanted(true);

    const [un1a, un1b, un2a, un2b] = [vi.fn(), vi.fn(), vi.fn(), vi.fn()];
    [un1a, un1b, un2a, un2b].forEach((u, i) => t.pendingSubs[i]!.resolve(u));
    await flush();
    expect(un1a).toHaveBeenCalledTimes(1);
    expect(un1b).toHaveBeenCalledTimes(1);
    expect(t.calls.map((c) => c.on)).toEqual([false, true]);

    const id = t.calls[1]!.id;
    t.workerSubs[0]!.cb(id, [worker("ghost")]);
    t.workerSubs[1]!.cb(id, [worker("real")]);
    expect(t.workersLog).toEqual([[worker("real")]]);

    clean2();
    expect(un2a).toHaveBeenCalledTimes(1);
    expect(un2b).toHaveBeenCalledTimes(1);
    await flush();
    expect(t.calls.map((c) => c.on)).toEqual([false, true, false]);
  });
});
