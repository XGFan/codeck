import type { PlanLimit, PlanUsage } from "./bindings";
import type { Api } from "./api";
import { appError } from "./mock-ccrpc";
import { DAY, NOW } from "@/mocks/data/builders";

export interface PlanPreset {
  /** Response returned by `getPlanUsage`: a snapshot or `null` (no subscription quota). */
  value?: PlanUsage | null;
  /** When set, `getPlanUsage` rejects with this message instead of resolving. */
  error?: string;
  /** When true, `getPlanUsage` stays pending until `window.__cvPlan.release()` is called. */
  hold?: boolean;
}

export interface PlanProbe {
  /** Total `getPlanUsage` calls. */
  calls: number;
  /** `getPlanUsage(true)` calls. */
  refreshCalls: number;
  /** Sets the snapshot (or `null`) returned by subsequent `getPlanUsage` calls and clears any error. */
  set(usage: PlanUsage | null): void;
  /** Replaces `limits` (and optional `plan` / `fetchedMs`) on the next response and clears any error. */
  setLimits(limits: PlanLimit[], opts?: { plan?: string | null; fetchedMs?: number }): void;
  /** Causes subsequent `getPlanUsage` calls to reject with `message`. */
  setError(message: string): void;
  /** Holds subsequent `getPlanUsage` calls pending until `release()` (or `hold(false)`). */
  hold(on: boolean): void;
  /** Resolves/rejects any held `getPlanUsage` calls with the current configured response. */
  release(): void;
  /** Restores default snapshot, clears error/hold, and resets call counters. */
  reset(): void;
}

declare global {
  interface Window {
    /** Optional E2E preset read before the first `getPlanUsage` call (installed via `page.addInitScript`). */
    __cvPlanPreset?: PlanPreset;
    /** E2E spy and stub controller for `getPlanUsage`. */
    __cvPlan?: PlanProbe;
  }
}

const LATENCY_MS = 30;

function defaultUsage(): PlanUsage {
  return {
    plan: "max",
    fetchedMs: Date.now(),
    limits: [
      { kind: "session", model: null, percent: 16, resetsMs: NOW + (2 * 60 + 20) * 60_000 },
      { kind: "weeklyAll", model: null, percent: 31, resetsMs: NOW + 3 * DAY },
      { kind: "weeklyScoped", model: "Fable", percent: 45, resetsMs: NOW + 3 * DAY },
    ],
  };
}

type PlanApi = Pick<Api, "getPlanUsage">;

export function createMockPlan(): PlanApi {
  const preset = typeof window !== "undefined" ? window.__cvPlanPreset : undefined;
  let customValue: PlanUsage | null | undefined = preset && "value" in preset ? preset.value : undefined;
  let errorMsg: string | null = preset?.error ?? null;
  let holding = !!preset?.hold;
  const waiters: Array<() => void> = [];

  const produce = (): PlanUsage | null => {
    if (errorMsg != null) throw appError("io", errorMsg);
    if (customValue !== undefined) return structuredClone(customValue);
    return defaultUsage();
  };

  const flush = () => {
    while (waiters.length > 0) waiters.shift()!();
  };

  const probe: PlanProbe = {
    calls: 0,
    refreshCalls: 0,
    set(usage) {
      errorMsg = null;
      customValue = usage ? structuredClone(usage) : null;
    },
    setLimits(limits, opts) {
      errorMsg = null;
      const base = customValue ?? defaultUsage();
      customValue = {
        plan: opts && "plan" in opts ? (opts.plan ?? null) : base.plan,
        fetchedMs: opts?.fetchedMs ?? Date.now(),
        limits: structuredClone(limits),
      };
    },
    setError(message) {
      errorMsg = message;
    },
    hold(on) {
      holding = on;
      if (!on) flush();
    },
    release() {
      holding = false;
      flush();
    },
    reset() {
      customValue = undefined;
      errorMsg = null;
      holding = false;
      probe.calls = 0;
      probe.refreshCalls = 0;
      flush();
    },
  };

  if (typeof window !== "undefined") window.__cvPlan = probe;

  return {
    getPlanUsage: (refresh) => {
      probe.calls++;
      if (refresh) probe.refreshCalls++;
      return new Promise((resolve, reject) => {
        const settle = () => {
          try {
            resolve(produce());
          } catch (e) {
            reject(e);
          }
        };
        if (holding) waiters.push(settle);
        else setTimeout(settle, LATENCY_MS);
      });
    },
  };
}
