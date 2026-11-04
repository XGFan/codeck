import { useCallback, useLayoutEffect, useRef, type RefObject } from "react";

const PIN_THRESHOLD = 48;

/**
 * Keeps a scroller at the bottom while its content grows, as long as the user was at the bottom.
 * `ref` is the scroller, `content` the element whose size changes. Returns the onScroll handler and a
 * way to pin again (e.g. when the user sends a prompt).
 */
export function useFollow(ref: RefObject<HTMLElement | null>, content: RefObject<HTMLElement | null>) {
  const pinned = useRef(true);
  const toBottom = useCallback(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [ref]);

  useLayoutEffect(() => {
    const c = content.current;
    if (!c || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      if (pinned.current) toBottom();
    });
    ro.observe(c);
    return () => ro.disconnect();
  }, [content, toBottom]);

  useLayoutEffect(() => {
    if (pinned.current) toBottom();
  });

  const onScroll = useCallback(() => {
    const el = ref.current;
    if (el) pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight <= PIN_THRESHOLD;
  }, [ref]);

  const pin = useCallback(() => {
    pinned.current = true;
    toBottom();
  }, [toBottom]);

  return { onScroll, pin, pinned };
}
