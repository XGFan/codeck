import { ChevronDown, ChevronUp, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FindMatch, FindRequest, TranscriptScope } from "@/ipc/bindings";
import { isComposingKey, useHotkey } from "@/lib/hotkeys";
import { useFind } from "@/queries";
import { MAIN_SCOPE, useUi } from "@/state/ui";
import { scrollToNode } from "../transcript/Transcript";
import { requestFullOutput } from "../transcript/tools";
import { errorText } from "@/lib/errors";

const NO_MATCHES: FindMatch[] = [];
const HIGHLIGHT = "cv-find";

/** Scope of the pane the user last interacted with: the Subagent panel's innermost run, else the main transcript. */
function scopeFromTarget(target: Element | null): TranscriptScope {
  const id = target?.closest("[data-scope]")?.getAttribute("data-scope");
  return id && id !== "main" ? { kind: "subagent", agentId: id } : MAIN_SCOPE;
}

/** ⌘F page-find bar (top of the conversation): `find_in_session`, stepping, auto-expansion via `scrollToNode`. */
export function FindBar() {
  const open = useUi((s) => s.findOpen);
  const sessionId = useUi((s) => s.sessionId);
  const query = useUi((s) => s.findQuery);
  const index = useUi((s) => s.findIndex);
  const showHidden = useUi((s) => s.showHidden);
  const branchChoices = useUi((s) => s.branchChoices);
  const panelOpen = useUi((s) => s.panelStack.length > 0);
  // IME composition (e.g. pinyin) puts unconfirmed text in the input; find only confirmed text.
  const [composing, setComposing] = useState(false);
  const [settledQuery, setSettledQuery] = useState(query);
  useEffect(() => {
    if (!composing) setSettledQuery(query);
  }, [query, composing]);
  const [scope, setScope] = useState<TranscriptScope>(MAIN_SCOPE);
  const inputRef = useRef<HTMLInputElement>(null);
  const lastScope = useRef<TranscriptScope>(MAIN_SCOPE);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (e.target instanceof Element && e.target.closest("[data-scope]"))
        lastScope.current = scopeFromTarget(e.target);
    };
    window.addEventListener("pointerdown", onDown, true);
    return () => window.removeEventListener("pointerdown", onDown, true);
  }, []);

  const canFind = sessionId != null;
  useHotkey(
    "Meta+f",
    () => {
      if (!canFind) return;
      const st = useUi.getState();
      if (!st.findOpen) setScope(st.panelStack.length ? lastScope.current : MAIN_SCOPE);
      st.setFindOpen(true);
      requestAnimationFrame(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
      });
    },
    { enableInInputs: true },
  );

  const show = open && canFind;
  const activeScope = panelOpen ? scope : MAIN_SCOPE;

  const go = useCallback(
    (list: FindMatch[], i: number) => {
      const m = list[i];
      if (!m) return;
      useUi.getState().setFindIndex(i);
      const loc = m.location;
      const toolUseId = loc.kind === "toolInput" || loc.kind === "toolOutput" ? loc.toolUseId : undefined;
      // A14e: output previews may be truncated; make the renderer load the full text before we point at it.
      if (loc.kind === "toolOutput") requestFullOutput(m.nodeId, loc.toolUseId);
      scrollToNode({
        scope: activeScope,
        nodeId: m.nodeId,
        toolUseId,
        thinking: loc.kind === "thinking",
        flashMs: null,
      });
    },
    [activeScope],
  );

  // Debounced search through react-query, so live appends (`useBackendEvents`) refresh the matches.
  const [debounced, setDebounced] = useState(settledQuery);
  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(settledQuery), 180);
    return () => window.clearTimeout(t);
  }, [settledQuery]);
  const req = useMemo<FindRequest | null>(
    () =>
      show && sessionId && debounced.trim()
        ? { sessionId, scope: activeScope, branchChoices, includeHidden: showHidden, query: debounced }
        : null,
    [show, sessionId, debounced, activeScope, branchChoices, showHidden],
  );
  const find = useFind(req);
  // Results of the request for the confirmed, debounced query (not the previous request's placeholder).
  const current = debounced === settledQuery && !find.isPlaceholderData ? find.data : undefined;
  const matches = current?.matches ?? NO_MATCHES;
  const error = debounced === settledQuery && req ? find.error : null;

  // A fresh request jumps to its first match; a live refresh of the same request keeps the position.
  const goRef = useRef(go);
  goRef.current = go;
  const jumped = useRef<FindRequest | null>(null);
  useEffect(() => {
    if (!current || !req || jumped.current === req) return;
    jumped.current = req;
    useUi.getState().setFindIndex(0);
    goRef.current(current.matches, 0);
  }, [current, req]);

  // Closing (or leaving the session) drops the persistent highlight and the text marks.
  useEffect(() => {
    if (!show) {
      jumped.current = null;
      setComposing(false);
      useUi.getState().setHighlight(null);
    }
  }, [show]);
  useTextMarks(show ? settledQuery : "");

  const at = Math.min(index, matches.length - 1);
  const step = (d: 1 | -1) => {
    if (!matches.length) return;
    go(matches, (at + d + matches.length) % matches.length);
  };

  if (!show) return null;
  const failure = error ? `查找失败：${errorText(error)}` : "";
  return (
    <div
      data-testid="find"
      role="search"
      className="flex shrink-0 items-center gap-2 border-b border-border bg-list py-1.5 pr-5 pl-6"
    >
      <input
        ref={inputRef}
        data-testid="find-input"
        autoFocus
        value={query}
        placeholder={activeScope.kind === "main" ? "在对话中查找" : "在 Subagent 中查找"}
        aria-label="页内查找"
        onChange={(e) => useUi.getState().setFindQuery(e.target.value)}
        onCompositionStart={() => setComposing(true)}
        onCompositionEnd={() => setComposing(false)}
        onKeyDown={(e) => {
          if (isComposingKey(e.nativeEvent)) return;
          if (e.key === "Enter") {
            e.preventDefault();
            step(e.shiftKey ? -1 : 1);
          } else if (e.key === "ArrowDown") {
            e.preventDefault();
            step(1);
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            step(-1);
          } else if (e.key === "Escape") {
            e.preventDefault();
            useUi.getState().setFindOpen(false);
          }
        }}
        className="h-7 min-w-0 max-w-80 flex-1 rounded-md border border-border bg-ground px-2.5 text-[13px] outline-none focus:border-accent"
      />
      <span data-testid="find-count" className="min-w-12 text-[12px] text-secondary tabular-nums">
        {!current || error ? "" : matches.length === 0 ? "无结果" : `${at + 1} / ${matches.length}`}
      </span>
      {failure && (
        <span role="alert" data-testid="find-error" className="min-w-0 truncate text-[12px] text-error">
          {failure}
        </span>
      )}
      <button type="button" aria-label="上一个" disabled={!matches.length} onClick={() => step(-1)} className={iconBtn}>
        <ChevronUp size={14} strokeWidth={1.8} aria-hidden />
      </button>
      <button type="button" aria-label="下一个" disabled={!matches.length} onClick={() => step(1)} className={iconBtn}>
        <ChevronDown size={14} strokeWidth={1.8} aria-hidden />
      </button>
      <button
        type="button"
        aria-label="关闭查找"
        onClick={() => useUi.getState().setFindOpen(false)}
        className={iconBtn}
      >
        <X size={14} strokeWidth={1.8} aria-hidden />
      </button>
    </div>
  );
}

const iconBtn =
  "flex size-6 items-center justify-center rounded text-secondary hover:bg-selection hover:text-text disabled:opacity-40 disabled:hover:bg-transparent";

/** Marks every occurrence of `query` in the rendered transcript text via the CSS Custom Highlight API (no DOM edits). */
function useTextMarks(query: string) {
  useEffect(() => {
    const reg =
      typeof CSS !== "undefined" ? (CSS as unknown as { highlights?: Map<string, unknown> }).highlights : undefined;
    const HL = (globalThis as unknown as { Highlight?: new (...r: Range[]) => unknown }).Highlight;
    if (!reg || !HL) return;
    const q = query.trim().toLowerCase();
    if (!q) {
      reg.delete(HIGHLIGHT);
      return;
    }
    let raf = 0;
    const paint = () => {
      const ranges: Range[] = [];
      for (const root of document.querySelectorAll("[data-scope]")) {
        const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
          const hay = n.nodeValue!.toLowerCase();
          for (let at = hay.indexOf(q); at >= 0; at = hay.indexOf(q, at + q.length)) {
            const r = document.createRange();
            r.setStart(n, at);
            r.setEnd(n, at + q.length);
            ranges.push(r);
          }
        }
      }
      reg.set(HIGHLIGHT, new HL(...ranges));
    };
    const schedule = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(paint);
    };
    schedule();
    const mo = new MutationObserver(schedule);
    mo.observe(document.body, { childList: true, subtree: true, characterData: true });
    return () => {
      cancelAnimationFrame(raf);
      mo.disconnect();
      reg.delete(HIGHLIGHT);
    };
  }, [query]);
}
