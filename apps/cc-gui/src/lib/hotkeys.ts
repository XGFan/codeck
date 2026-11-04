import { useEffect, useRef } from "react";

interface HotkeyOptions {
  enableInInputs?: boolean;
}

/** combo 形如 "Meta+K"、"Meta+Shift+F"、"Escape"；修饰键顺序无关，大小写不敏感。 */
function matches(e: KeyboardEvent, combo: string): boolean {
  const parts = combo.split("+").map((p) => p.toLowerCase());
  const key = parts[parts.length - 1];
  const mods = new Set(parts.slice(0, -1));
  return (
    e.key.toLowerCase() === key &&
    e.metaKey === mods.has("meta") &&
    e.ctrlKey === mods.has("ctrl") &&
    e.altKey === mods.has("alt") &&
    e.shiftKey === mods.has("shift")
  );
}

/** The key goes to an IME composition (e.g. Esc cancelling pinyin candidates); WebKit reports keyCode 229. */
export function isComposingKey(e: KeyboardEvent): boolean {
  return e.isComposing || e.keyCode === 229;
}

/** `onEscapeKeyDown` for dialogs with text inputs: Esc during a composition only cancels the composition. */
export function keepOpenWhileComposing(e: KeyboardEvent) {
  if (isComposingKey(e)) e.preventDefault();
}

function inEditable(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  return el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable;
}

export function useHotkey(
  combo: string,
  handler: (e: KeyboardEvent) => void,
  { enableInInputs = false }: HotkeyOptions = {},
) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (isComposingKey(e) || (!enableInInputs && inEditable(e.target))) return;
      if (!matches(e, combo)) return;
      e.preventDefault();
      ref.current(e);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [combo, enableInInputs]);
}
