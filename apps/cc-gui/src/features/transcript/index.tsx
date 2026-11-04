import { Loader2 } from "lucide-react";
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useStore } from "@/state/store";
import { buildTurns } from "./model";
import { RowList } from "./rows";
import { useFollow } from "./scroll";

export { AgentCard, RowList } from "./rows";
export { ToolCallView } from "./tools";
export { buildRows, buildTurns, findCall, type Row, type ToolCall, type Turn } from "./model";
export { useFollow } from "./scroll";

/** Distance from the top that loads the older page. */
const TOP_THRESHOLD = 80;

/** The Main Line of connection `conn`, grouped by turn; follows the bottom and pages older history at the top. */
export function Transcript({ conn }: { conn: string }) {
  const items = useStore((s) => s.conns[conn]?.items);
  const loaded = useStore((s) => s.conns[conn]?.loaded ?? false);
  const more = useStore((s) => s.conns[conn]?.more ?? false);
  const turns = useMemo(() => buildTurns(items ?? []), [items]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const follow = useFollow(scrollRef, contentRef);
  const [loadingOlder, setLoadingOlder] = useState(false);
  /** The first row on screen and its offset before an older page is prepended. */
  const anchor = useRef<{ key: string; top: number } | null>(null);

  const loadOlder = useCallback(() => {
    const el = scrollRef.current;
    if (!el || loadingOlder || !more) return;
    const first = el.querySelector<HTMLElement>("[data-row-key]");
    anchor.current = first ? { key: first.dataset.rowKey!, top: first.getBoundingClientRect().top } : null;
    setLoadingOlder(true);
    void useStore
      .getState()
      .loadOlder(conn)
      .finally(() => setLoadingOlder(false));
  }, [conn, loadingOlder, more]);

  // Keep the row that was at the top in place once older rows are prepended.
  useLayoutEffect(() => {
    const a = anchor.current;
    const el = scrollRef.current;
    if (!a || !el || loadingOlder) return;
    anchor.current = null;
    const now = el.querySelector<HTMLElement>(`[data-row-key="${CSS.escape(a.key)}"]`);
    if (now) el.scrollTop += now.getBoundingClientRect().top - a.top;
  }, [turns, loadingOlder]);

  // A new prompt of the user's own brings the view back to the bottom.
  const lastKey = items?.[items.length - 1]?.key;
  const last = items?.[items.length - 1];
  const lastIsPrompt = last?.kind === "block" && last.block.kind === "user" && !last.block.queued;
  const seenLast = useRef(lastKey);
  useLayoutEffect(() => {
    if (lastKey === seenLast.current) return;
    seenLast.current = lastKey;
    if (lastIsPrompt) follow.pin();
  }, [lastKey, lastIsPrompt, follow]);

  const onScroll = () => {
    follow.onScroll();
    const el = scrollRef.current;
    if (el && el.scrollTop < TOP_THRESHOLD && more && loaded) loadOlder();
  };

  return (
    <div ref={scrollRef} onScroll={onScroll} data-testid="transcript" className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-6">
      <div ref={contentRef} className="mx-auto flex max-w-[820px] flex-col gap-[18px] py-5">
        {!loaded ? (
          <div className="flex items-center justify-center gap-2 py-10 text-[13px] text-secondary" role="status">
            <Loader2 size={14} className="animate-spin" aria-hidden />
            加载中…
          </div>
        ) : (
          <>
            {more && (
              <button
                type="button"
                data-testid="load-older"
                onClick={loadOlder}
                disabled={loadingOlder}
                className="flex items-center justify-center gap-1.5 self-center rounded-full px-3 py-1 text-[12px] text-accent hover:bg-selection disabled:text-secondary"
              >
                {loadingOlder && <Loader2 size={12} className="animate-spin" aria-hidden />}
                {loadingOlder ? "加载中…" : "加载更早的消息"}
              </button>
            )}
            {turns.length === 0 && <div className="py-10 text-center text-[13px] text-secondary">还没有消息</div>}
            {turns.map((t) => (
              <section key={t.key} data-testid="turn" className="flex flex-col gap-2">
                <RowList conn={conn} rows={t.rows} />
              </section>
            ))}
          </>
        )}
      </div>
    </div>
  );
}
