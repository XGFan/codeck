const HIDE_AFTER_MS = 900;

/**
 * Shows a scroller's thumb (see index.css) only while it moves — like macOS overlay scrollbars, whatever the
 * system "Show scroll bars" setting or input device. WebKit repaints a custom scrollbar on an inline style
 * change but not on an attribute or class change, hence the inline custom property.
 */
export function installScrollbarAutoHide() {
  const timers = new WeakMap<HTMLElement, number>();
  document.addEventListener(
    "scroll",
    (e) => {
      const el = e.target instanceof HTMLElement ? e.target : document.documentElement;
      el.style.setProperty("--scroll-thumb", "var(--color-scroll-thumb)");
      window.clearTimeout(timers.get(el));
      timers.set(el, window.setTimeout(() => el.style.removeProperty("--scroll-thumb"), HIDE_AFTER_MS));
    },
    { capture: true, passive: true },
  );
}
