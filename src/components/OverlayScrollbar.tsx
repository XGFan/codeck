import { type RefObject, useEffect, useRef } from "react";

const HIDE_AFTER_MS = 900;
const MIN_THUMB = 24;

/**
 * A scrollbar thumb drawn over the scroller's right edge, for a scroller whose own scrollbar is hidden
 * (`no-scrollbar`): a WebKit custom scrollbar always takes its width out of the content, this one takes none.
 * Shows while scrolling and while the pointer is on the strip; drag the thumb to scroll, click the strip to page,
 * the wheel over it scrolls the scroller. `onUserScroll` runs on each of those, as the scroller's own pointer and
 * wheel handlers would (the strip is outside it). Place it in a positioned parent that the scroller fills.
 */
export function OverlayScrollbar({
  scrollRef,
  onUserScroll,
  testId,
}: {
  scrollRef: RefObject<HTMLElement | null>;
  onUserScroll?: () => void;
  testId?: string;
}) {
  const strip = useRef<HTMLDivElement>(null);
  const thumb = useRef<HTMLDivElement>(null);
  const state = useRef({ hover: false, drag: false, timer: 0, frame: 0 });

  useEffect(() => {
    const el = scrollRef.current;
    const bar = strip.current;
    const th = thumb.current;
    if (!el || !bar || !th) return;
    const s = state.current;
    const layout = () => {
      s.frame = 0;
      const { scrollHeight: sh, clientHeight: ch, scrollTop: st } = el;
      // The strip spans the scroller (and reads 0 while hidden).
      const track = ch;
      if (sh <= ch + 1 || track <= 0) {
        // Hidden under the pointer, the strip may get no pointerleave.
        bar.hidden = true;
        s.hover = false;
        delete bar.dataset.visible;
        return;
      }
      bar.hidden = false;
      const h = Math.max(MIN_THUMB, (track * ch) / sh);
      th.style.height = `${h}px`;
      // A bounce at either end (trackpad rubber-banding) can put scrollTop past its range; the thumb stays in the track.
      const at = Math.min(1, Math.max(0, st / (sh - ch)));
      th.style.transform = `translateY(${at * (track - h)}px)`;
    };
    const schedule = () => {
      if (!s.frame) s.frame = requestAnimationFrame(layout);
    };
    const onScroll = () => {
      schedule();
      bar.dataset.visible = "true";
      window.clearTimeout(s.timer);
      s.timer = window.setTimeout(() => {
        if (!s.hover && !s.drag) delete bar.dataset.visible;
      }, HIDE_AFTER_MS);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    // Content grows or shrinks without a scroll event (new rows, measurements, a resized window).
    const ro = new ResizeObserver(schedule);
    ro.observe(el);
    for (const c of el.children) ro.observe(c);
    const mo = new MutationObserver(() => {
      for (const c of el.children) ro.observe(c);
      schedule();
    });
    mo.observe(el, { childList: true });
    layout();
    return () => {
      el.removeEventListener("scroll", onScroll);
      ro.disconnect();
      mo.disconnect();
      window.clearTimeout(s.timer);
      cancelAnimationFrame(s.frame);
      s.frame = 0;
    };
  }, [scrollRef]);

  const setHover = (on: boolean) => {
    const s = state.current;
    s.hover = on;
    const bar = strip.current;
    if (!bar) return;
    if (on) bar.dataset.visible = "true";
    else if (!s.drag) delete bar.dataset.visible;
  };

  const onThumbDown = (e: React.PointerEvent<HTMLDivElement>) => {
    const el = scrollRef.current;
    const bar = strip.current;
    const th = thumb.current;
    if (e.button !== 0 || !el || !bar || !th) return;
    e.preventDefault();
    e.stopPropagation();
    const s = state.current;
    s.drag = true;
    bar.dataset.dragging = "true";
    th.setPointerCapture(e.pointerId);
    onUserScroll?.();
    // Where on the thumb it was grabbed; the thumb's top follows the pointer, mapped with the current heights
    // (the virtual list measures rows, so the content height changes during a drag).
    const grab = e.clientY - th.getBoundingClientRect().top;
    const move = (m: PointerEvent) => {
      const free = el.clientHeight - th.offsetHeight;
      const top = m.clientY - bar.getBoundingClientRect().top - grab;
      el.scrollTop = (top / Math.max(1, free)) * (el.scrollHeight - el.clientHeight);
    };
    // Fires after pointerup / pointercancel and whenever the capture is lost otherwise.
    const end = () => {
      th.removeEventListener("pointermove", move);
      th.removeEventListener("lostpointercapture", end);
      s.drag = false;
      delete bar.dataset.dragging;
      if (!s.hover) delete bar.dataset.visible;
    };
    th.addEventListener("pointermove", move);
    th.addEventListener("lostpointercapture", end);
  };

  // A click on the strip off the thumb pages toward the click, like a native track.
  const onStripDown = (e: React.PointerEvent<HTMLDivElement>) => {
    const el = scrollRef.current;
    const th = thumb.current;
    if (e.button !== 0 || !el || !th) return;
    e.preventDefault();
    onUserScroll?.();
    const above = e.clientY < th.getBoundingClientRect().top;
    el.scrollBy({ top: (above ? -1 : 1) * el.clientHeight * 0.9 });
  };

  return (
    <div
      ref={strip}
      aria-hidden
      data-testid={testId}
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
      onPointerDown={onStripDown}
      onWheel={(e) => {
        onUserScroll?.();
        scrollRef.current?.scrollBy({ top: e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY });
      }}
      // Clips the thumb: past the strip's end it would overflow the page, whose (10px) scrollbar then shifts the layout.
      className="group/sb absolute inset-y-0 right-0 z-10 w-2.5 overflow-hidden select-none"
    >
      <div
        ref={thumb}
        data-testid={testId && `${testId}-thumb`}
        onPointerDown={onThumbDown}
        className="absolute top-0 right-0.5 w-1.5 rounded-full bg-(--color-scroll-thumb) opacity-0 transition-opacity duration-200 group-data-visible/sb:opacity-100 group-data-dragging/sb:opacity-100"
      />
    </div>
  );
}
