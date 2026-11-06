import { useEffect, useRef, useState } from "react";
import { isComposingKey, popupOpen } from "@/lib/hotkeys";
import { selectOverlayOpen, useUi } from "@/state/ui";
import { answerError } from "./answers";
import type { ConnState } from "./conn";
import { useConnect } from "./store";

/** Sends an answer to a request; answered, the card leaves at once. */
export function useAnswer(c: ConnState) {
  const command = useConnect((s) => s.command);
  const dropRequest = useConnect((s) => s.dropRequest);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const send = async (msg: Record<string, unknown>) => {
    setBusy(true);
    setError(null);
    const r = await command(c.conn, msg);
    setBusy(false);
    // Answered: off screen now, so the next card gets the keys (ccrpc ends it on the tool's result).
    if (r.success) dropRequest(c.conn, String(msg.request_id));
    else setError(answerError(r.error));
    return r.success;
  };
  return { busy, error, send };
}

/**
 * Number keys pick the n-th button of the first card, only while focus is inside the input area (a card
 * button, e.g. after Enter while waiting) and not in an editable field: reading the transcript with a card
 * pending must not answer it.
 */
export function useNumberKeys(enabled: boolean, actions: (() => void)[]) {
  const ref = useRef(actions);
  ref.current = actions;
  useEffect(() => {
    if (!enabled) return;
    const on = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (!el?.closest?.("[data-connect-panel]")) return;
      if (el.tagName === "TEXTAREA" || el.tagName === "INPUT" || el.tagName === "SELECT" || el.isContentEditable)
        return;
      if (isComposingKey(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (selectOverlayOpen(useUi.getState()) || popupOpen()) return;
      const n = Number(e.key);
      if (n >= 1 && n <= ref.current.length) {
        e.preventDefault();
        ref.current[n - 1]!();
      }
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [enabled]);
}
