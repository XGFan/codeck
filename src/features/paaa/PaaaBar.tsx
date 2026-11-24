import { ChevronDown, ChevronRight } from "lucide-react";
import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";
import { formatDuration, formatTokens } from "@/lib/format";
import type { PaaaWorker, PaaaWorkerState } from "./protocol";
import { usePaaa } from "./store";

const STATE_META: Record<PaaaWorkerState, { label: string; dot: string; text: string }> = {
  running: { label: "运行中", dot: "bg-live", text: "text-live" },
  asking: { label: "等你回答", dot: "bg-live-waiting", text: "text-live-waiting" },
  idle: { label: "空闲", dot: "bg-secondary/50", text: "text-secondary" },
  dormant: { label: "休眠", dot: "bg-secondary/40", text: "text-secondary" },
  taken: { label: "接管中", dot: "bg-secondary/60", text: "text-secondary" },
  dead: { label: "已退出", dot: "bg-error", text: "text-error" },
};

const STATE_ORDER: PaaaWorkerState[] = ["asking", "running", "taken", "idle", "dormant", "dead"];

/** Current timestamp, ticking every second while `active`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [active]);
  return now;
}

/** One paaa Worker row, plus its pending question block when `state === "asking"`. */
function WorkerRow({ w, now, prefix }: { w: PaaaWorker; now: number; prefix?: boolean }) {
  const meta = STATE_META[w.state];
  const active = w.state === "running" || w.state === "asking";
  const elapsed = active && w.startedAt != null ? formatDuration(Math.max(0, now - w.startedAt), "detail") : "";
  const title = w.title ?? w.task;

  return (
    <div data-testid="paaa-worker" data-state={w.state} className="flex shrink-0 flex-col">
      <div className="flex min-w-0 items-center gap-2">
        {prefix && <span className="shrink-0 font-mono text-[11px] font-medium text-secondary">paaa</span>}
        <span className={cn("size-1.5 shrink-0 rounded-full", meta.dot)} aria-hidden />
        <span className="shrink-0 font-medium text-text">{w.name}</span>
        <span className={cn("shrink-0", meta.text)}>{meta.label}</span>
        {elapsed && <span className="shrink-0 text-secondary tabular-nums">{elapsed}</span>}
        {w.tokens > 0 && <span className="shrink-0 text-secondary tabular-nums">{formatTokens(w.tokens)} token</span>}
        {w.ctx != null && (
          <span className="shrink-0 text-secondary tabular-nums" title="上下文占用">
            {Math.round(w.ctx)}%
          </span>
        )}
        {title && (
          <span className="min-w-0 truncate text-secondary" title={title}>
            {title}
          </span>
        )}
        {w.activity && (
          <span className="min-w-0 truncate text-secondary/80" title={w.activity}>
            · {w.activity}
          </span>
        )}
      </div>
      {w.state === "asking" && w.pending && (
        <div
          data-testid="paaa-pending"
          tabIndex={0}
          aria-label={`${w.name} 待回答的问题`}
          className="mt-1.5 max-h-24 overflow-y-auto rounded-lg border border-border border-l-2 border-l-live-waiting bg-ground px-2.5 py-1.5 text-[12px] leading-relaxed break-words whitespace-pre-wrap text-text select-text focus:outline-none focus-visible:ring-1 focus-visible:ring-live-waiting"
        >
          {w.pending}
        </div>
      )}
    </div>
  );
}

/**
 * Read-only status bar for the paaa Workers owned by `sessionId`.
 * Renders nothing when the Session has no paaa Workers.
 */
export function PaaaBar({ sessionId }: { sessionId: string | null }) {
  const workers = usePaaa((s) => s.workers);
  const own = sessionId ? workers.filter((w) => w.owner === sessionId) : [];

  const askingKeys = own.filter((w) => w.state === "asking").map((w) => `${w.id}\0${w.pending ?? ""}`);
  const [open, setOpen] = useState(() => askingKeys.length > 0);
  const [prev, setPrev] = useState<{ sessionId: string | null; keys: string[] }>({
    sessionId,
    keys: askingKeys,
  });

  const keysChanged =
    prev.sessionId !== sessionId ||
    prev.keys.length !== askingKeys.length ||
    prev.keys.some((k, i) => k !== askingKeys[i]);

  if (keysChanged) {
    if (prev.sessionId !== sessionId) {
      setOpen(askingKeys.length > 0);
    } else if (askingKeys.some((k) => !prev.keys.includes(k))) {
      setOpen(true);
    }
    setPrev({ sessionId, keys: askingKeys });
  }

  const hasTicking =
    (own.length === 1 || open) &&
    own.some((w) => (w.state === "running" || w.state === "asking") && w.startedAt != null);
  const now = useNow(hasTicking);

  if (own.length === 0) return null;

  if (own.length === 1) {
    return (
      <div
        data-testid="paaa-workers"
        data-paaa-bar=""
        className="flex w-full flex-col rounded-2xl border border-border bg-ground px-3.5 py-2 text-[12px] shadow-[0_2px_10px_rgba(0,0,0,0.04)]"
      >
        <WorkerRow w={own[0]!} now={now} prefix />
      </div>
    );
  }

  const counts = STATE_ORDER.map((state) => ({
    state,
    n: own.filter((w) => w.state === state).length,
  })).filter((c) => c.n > 0);

  return (
    <div
      data-testid="paaa-workers"
      data-paaa-bar=""
      className="flex w-full flex-col rounded-2xl border border-border bg-ground px-3.5 py-2 text-[12px] shadow-[0_2px_10px_rgba(0,0,0,0.04)]"
    >
      <button
        type="button"
        data-testid="paaa-toggle"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 text-left text-secondary hover:text-text"
      >
        <span className="shrink-0 font-mono text-[11px] font-medium text-secondary">paaa</span>
        <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2">
          {counts.map(({ state, n }, i) => {
            const meta = STATE_META[state];
            return (
              <span key={state} className="inline-flex items-center gap-1.5">
                {i > 0 && (
                  <span className="text-secondary/60" aria-hidden>
                    ·
                  </span>
                )}
                <span className={cn("size-1.5 rounded-full", meta.dot)} aria-hidden />
                <span className={meta.text}>
                  {n} {meta.label}
                </span>
              </span>
            );
          })}
        </span>
        <span className="flex shrink-0 items-center gap-1 text-[11px] text-secondary">
          {open ? "收起" : "展开"}
          {open ? <ChevronDown size={13} aria-hidden /> : <ChevronRight size={13} aria-hidden />}
        </span>
      </button>
      {open && (
        <div className="mt-1.5 flex max-h-[45vh] flex-col gap-1.5 overflow-y-auto border-t border-border pt-1.5">
          {own.map((w) => (
            <WorkerRow key={w.id} w={w} now={now} />
          ))}
        </div>
      )}
    </div>
  );
}
