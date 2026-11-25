import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { api } from "@/ipc";
import type { CycleRule } from "@/lib/cycle";
import { queryKeys } from "@/queries/hooks";
import { useUi } from "@/state/ui";
import {
  cycleRuleFrom,
  derivePlanState,
  PLAN_CYCLE_ANCHOR_KEY,
  resolvePlanAnchor,
  sanitizeAnchorMs,
  type PlanState,
} from "./model";

const PLAN_STALE_MS = 60_000;
const PLAN_POLL_MS = 5 * 60_000;
const PLAN_ENABLED = import.meta.env.VITE_IPC !== "remote";

function loadSavedAnchor(): number | null {
  try {
    const raw = localStorage.getItem(PLAN_CYCLE_ANCHOR_KEY);
    if (raw == null) return null;
    const valid = sanitizeAnchorMs(raw);
    if (valid == null) localStorage.removeItem(PLAN_CYCLE_ANCHOR_KEY);
    return valid;
  } catch {
    return null;
  }
}

let savedAnchor: number | null = loadSavedAnchor();
const anchorListeners = new Set<() => void>();

function subscribeAnchor(cb: () => void): () => void {
  anchorListeners.add(cb);
  return () => anchorListeners.delete(cb);
}

function getSavedAnchor(): number | null {
  return savedAnchor;
}

function setSavedAnchor(next: number | null) {
  if (next === savedAnchor) return;
  savedAnchor = next;
  try {
    if (next == null) localStorage.removeItem(PLAN_CYCLE_ANCHOR_KEY);
    else localStorage.setItem(PLAN_CYCLE_ANCHOR_KEY, String(next));
  } catch {
    // Storage unavailable; keep in-memory value for this window.
  }
  anchorListeners.forEach((l) => l());
}

/** True only when the app window is both visible and focused (matches `features/attention`). */
function isForeground(): boolean {
  if (typeof document === "undefined") return true;
  return document.visibilityState === "visible" && document.hasFocus();
}

const fgListeners = new Set<() => void>();
let fgWatching = false;

function notifyForeground() {
  fgListeners.forEach((l) => l());
}

function subscribeForeground(cb: () => void): () => void {
  if (!fgWatching && typeof window !== "undefined" && typeof document !== "undefined") {
    fgWatching = true;
    window.addEventListener("focus", notifyForeground);
    window.addEventListener("blur", notifyForeground);
    document.addEventListener("visibilitychange", notifyForeground);
  }
  fgListeners.add(cb);
  return () => fgListeners.delete(cb);
}

/**
 * Shared subscription quota query for the sidebar ring, popover, stats strip, and cycle rule.
 * Stale after 60s; polls every 5 minutes only while the window is visible AND focused; refetches when
 * returning to the foreground if stale; `refresh()` calls `getPlanUsage(true)` and writes back to the same key.
 */
export function usePlanUsage(): {
  state: PlanState;
  anchorMs: number | null;
  isFetching: boolean;
  refresh: () => void;
} {
  const qc = useQueryClient();
  const foreground = useSyncExternalStore(subscribeForeground, isForeground, () => true);

  const q = useQuery({
    queryKey: queryKeys.planUsage,
    queryFn: () => api.getPlanUsage(false),
    staleTime: PLAN_STALE_MS,
    refetchInterval: foreground ? () => (isForeground() ? PLAN_POLL_MS : false) : false,
    refetchIntervalInBackground: false,
    // Handled explicitly below using both `window` focus/blur and `visibilitychange` (Tauri windows stay visible when blurred).
    refetchOnWindowFocus: false,
    retryOnMount: false,
    // Opening Stats / Settings right after a failed refresh should not immediately re-probe `claude`.
    refetchOnMount: (query) => query.state.error == null,
    enabled: PLAN_ENABLED,
  });

  // Reset TanStack Query's interval timer immediately when foreground state flips (e.g. window blur/focus).
  useEffect(() => {
    if (!PLAN_ENABLED || !foreground) return;
    const qs = qc.getQueryState(queryKeys.planUsage);
    if (!qs || qs.fetchStatus === "fetching") return;
    const lastAt = Math.max(qs.dataUpdatedAt, qs.errorUpdatedAt);
    if (lastAt > 0 && Date.now() - lastAt >= PLAN_STALE_MS) {
      void qc.refetchQueries({ queryKey: queryKeys.planUsage });
    }
  }, [qc, foreground]);

  const saved = useSyncExternalStore(subscribeAnchor, getSavedAnchor);
  const state = useMemo(() => derivePlanState({ data: q.data, error: q.error }), [q.data, q.error]);
  const { nextSaved, anchorMs } = useMemo(() => resolvePlanAnchor(state, saved), [state, saved]);

  useEffect(() => {
    if (nextSaved !== saved) setSavedAnchor(nextSaved);
  }, [nextSaved, saved]);

  const refresh = useCallback(() => {
    if (!PLAN_ENABLED || qc.isFetching({ queryKey: queryKeys.planUsage }) > 0) return;
    void qc
      .fetchQuery({
        queryKey: queryKeys.planUsage,
        queryFn: () => api.getPlanUsage(true),
        staleTime: 0,
      })
      .catch(() => {});
  }, [qc]);

  return { state, anchorMs, isFetching: q.isFetching, refresh };
}

/** Active weekly cycle rule: follows the subscription's weekly reset when available, otherwise Settings. */
export function useCycleRule(): { rule: CycleRule; anchorMs: number | null } {
  const { anchorMs } = usePlanUsage();
  const weeklyReset = useUi((s) => s.weeklyReset);
  const rule = useMemo(() => cycleRuleFrom(anchorMs, weeklyReset), [anchorMs, weeklyReset]);
  return { rule, anchorMs };
}

/** Wall-clock `Date` updated on every minute boundary so countdowns and cycle boundaries stay current. */
export function useMinuteNow(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const schedule = () => {
      const rem = 60_000 - (Date.now() % 60_000);
      timer = setTimeout(
        () => {
          setNow(new Date());
          schedule();
        },
        rem > 0 ? rem + 50 : 60_000,
      );
    };
    schedule();
    return () => clearTimeout(timer);
  }, []);
  return now;
}
