import { ImageIcon } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/cn";
import type { DraftImage } from "./drafts";
import { LEAD, snap, tokensIn, TRAIL, type Tok } from "./imageTokens";

/**
 * The textarea's text drawn under it (the textarea's own text is clear meanwhile): the command named at the start
 * in color, each image token as a block — the same characters, so the lines wrap where the textarea's do; the icon
 * sits on the token's ideographic space.
 */
export function MirrorText({
  text,
  highlight,
  selected,
  ghost = false,
}: {
  text: string;
  /** Characters at the start shown as a command. */
  highlight: number;
  /** The token the selection covers exactly (drawn selected). */
  selected: number | null;
  /** The blocks alone, the letters clear (an input method composes: the textarea shows its own text then). */
  ghost?: boolean;
}) {
  const out: React.ReactNode[] = [];
  let at = 0;
  const words = (s: string, from: number) => {
    const cut = Math.max(0, Math.min(s.length, highlight - from));
    if (cut > 0)
      out.push(
        <span key={`h${from}`} className={ghost ? undefined : "text-accent"}>
          {s.slice(0, cut)}
        </span>,
      );
    if (cut < s.length) out.push(s.slice(cut));
  };
  for (const t of tokensIn(text)) {
    words(text.slice(at, t.start), at);
    // The start mark and the gap before the block, outside it.
    out.push(text.slice(t.start, t.start + LEAD));
    out.push(
      <span
        key={`t${t.start}`}
        data-chip={t.n}
        className={cn(
          // Room inside the block, taken back by the margins: the letters stay where the textarea has them (the
          // padding reaches into the gaps around the block).
          "relative -mx-[3px] rounded-[5px] bg-code px-[3px] text-text shadow-[0_0_0_1px_var(--color-border)]",
          selected === t.n && "bg-selection text-accent shadow-[0_0_0_1.5px_var(--color-accent)]",
          ghost && "text-transparent",
        )}
      >
        {"\u3000"}
        <ImageIcon
          size={12}
          strokeWidth={1.8}
          className="pointer-events-none absolute top-1/2 left-[4px] -translate-y-1/2 text-secondary"
          aria-hidden
        />
        {text.slice(t.start + LEAD + 1, t.end - TRAIL)}
      </span>,
    );
    // The gap after the block, the end mark.
    out.push(text.slice(t.end - TRAIL, t.end));
    at = t.end;
  }
  words(text.slice(at), at);
  return <>{out}</>;
}

interface ChipBox {
  tok: Tok;
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Offset in `text` of a DOM position inside the mirror (its text nodes, in order, are the text). */
function textOffset(root: HTMLElement, node: Node, offset: number): number | null {
  if (!root.contains(node)) return null;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let sum = 0;
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n === node) return sum + offset;
    sum += n.textContent?.length ?? 0;
  }
  return node === root ? sum : null;
}

/** The mirror's DOM position of text offset `pos`. */
function domAt(root: HTMLElement, pos: number): { node: Node; offset: number } | null {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let sum = 0;
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const len = n.textContent?.length ?? 0;
    if (pos <= sum + len) return { node: n, offset: pos - sum };
    sum += len;
  }
  return null;
}

/**
 * Over the textarea, where the mirror draws the tokens: hovering one previews its image; a click selects it (⌫
 * removes it); dragging it moves it between words — the spot shown by a line, Esc cancels.
 */
export function ChipLayer({
  text,
  images,
  mirror,
  input,
  box,
  onMove,
}: {
  text: string;
  images: DraftImage[];
  mirror: RefObject<HTMLDivElement | null>;
  input: RefObject<HTMLTextAreaElement | null>;
  /** The positioned box holding the mirror and the textarea. */
  box: RefObject<HTMLDivElement | null>;
  onMove: (tok: Tok, to: number) => void;
}) {
  const [chips, setChips] = useState<ChipBox[]>([]);
  const [hover, setHover] = useState<number | null>(null);
  // The pointer leaving a token or its preview hides the preview a moment later: time to cross the gap onto it.
  const hideTimer = useRef<number | undefined>(undefined);
  const keep = () => window.clearTimeout(hideTimer.current);
  const hideSoon = () => {
    keep();
    hideTimer.current = window.setTimeout(() => setHover(null), HIDE_DELAY);
  };
  useEffect(() => keep, []);
  const [large, setLarge] = useState<{ n: number; img: DraftImage } | null>(null);
  const [drag, setDrag] = useState<{ tok: Tok; x: number; y: number; to: number | null; line: DOMRect | null } | null>(
    null,
  );
  const [scroll, setScroll] = useState(0);

  // Where the tokens are drawn now: after each change, on scroll and as the box resizes. An effect, not a layout
  // effect: mounted with the input box, the box's own ref is set only after this component's layout effects.
  useEffect(() => {
    const m = mirror.current;
    const b = box.current;
    const t = input.current;
    if (!m || !b) return;
    const measure = () => {
      const base = b.getBoundingClientRect();
      // Only what shows: a token scrolled out of the textarea has no box (it would sit over the transcript or the
      // send button).
      const view = t?.getBoundingClientRect();
      const toks = tokensIn(text);
      setChips(
        [...m.querySelectorAll<HTMLElement>("[data-chip]")].flatMap((el, i) => {
          const r = el.getClientRects()[0];
          const tok = toks[i];
          if (!r || !tok) return [];
          if (view && (r.bottom <= view.top + 2 || r.top >= view.bottom - 2)) return [];
          return [{ tok, left: r.left - base.left, top: r.top - base.top, width: r.width, height: r.height }];
        }),
      );
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(b);
    const onScroll = () => setScroll(t?.scrollTop ?? 0);
    t?.addEventListener("scroll", onScroll);
    return () => {
      ro.disconnect();
      t?.removeEventListener("scroll", onScroll);
    };
  }, [text, mirror, box, input, scroll]);

  const dragRef = useRef(drag);
  dragRef.current = drag;

  /** The text offset between words nearest the pointer, and the caret's box there. */
  const dropAt = (x: number, y: number): { to: number; line: DOMRect } | null => {
    const m = mirror.current;
    const t = input.current;
    const b = box.current;
    if (!m || !t || !b) return null;
    // Hit-test the mirror: for a moment the only thing here that takes the pointer (not the textarea, not the
    // tokens' hover boxes above it).
    const hits = [t, ...b.querySelectorAll<HTMLElement>("[data-testid=image-chip]")];
    const before = hits.map((el) => el.style.pointerEvents);
    hits.forEach((el) => (el.style.pointerEvents = "none"));
    m.style.pointerEvents = "auto";
    const r = document.caretRangeFromPoint?.(x, y);
    m.style.pointerEvents = "";
    hits.forEach((el, i) => (el.style.pointerEvents = before[i]!));
    const off = r ? textOffset(m, r.startContainer, r.startOffset) : null;
    if (off == null) return null;
    const to = snap(text, Math.min(off, text.length), 0);
    const at = domAt(m, to);
    if (!at) return null;
    const range = document.createRange();
    range.setStart(at.node, at.offset);
    range.collapse(true);
    const rect = range.getClientRects()[0] ?? range.getBoundingClientRect();
    const base = b.getBoundingClientRect();
    return { to, line: new DOMRect(rect.left - base.left, rect.top - base.top, 2, rect.height || 19) };
  };

  const start = (c: ChipBox, e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const x0 = e.clientX;
    const y0 = e.clientY;
    let moved = false;
    const onMove = (ev: PointerEvent) => {
      if (!moved && Math.hypot(ev.clientX - x0, ev.clientY - y0) < 4) return;
      moved = true;
      keep();
      setHover(null);
      const d = dropAt(ev.clientX, ev.clientY);
      setDrag({ tok: c.tok, x: ev.clientX, y: ev.clientY, to: d?.to ?? null, line: d?.line ?? null });
    };
    const end = (commit: boolean) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKey, true);
      const d = dragRef.current;
      setDrag(null);
      if (!moved) {
        // A click: the token selected, so ⌫ takes it.
        const el = input.current;
        el?.focus();
        el?.setSelectionRange(c.tok.start, c.tok.end);
        return;
      }
      if (commit && d?.to != null) onMovePiece(c.tok, d.to);
    };
    const onMovePiece = (tok: Tok, to: number) => onMoveRef.current(tok, to);
    const onUp = () => end(true);
    // Released outside the window, or the gesture taken over: nothing moves.
    const onCancel = () => {
      moved = true;
      dragRef.current = null;
      end(false);
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      ev.preventDefault();
      ev.stopImmediatePropagation();
      end(false);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKey, true);
  };
  const onMoveRef = useRef(onMove);
  onMoveRef.current = onMove;

  const shown = chips.find((c) => c.tok.n === hover);
  const img = shown ? images[shown.tok.n - 1] : undefined;
  return (
    <>
      {chips.map((c) => (
        <div
          key={`${c.tok.start}:${c.tok.n}`}
          role="img"
          aria-label={`图片 ${c.tok.n}`}
          data-testid="image-chip"
          data-n={c.tok.n}
          onPointerEnter={() => {
            if (dragRef.current) return;
            keep();
            setHover(c.tok.n);
          }}
          onPointerLeave={hideSoon}
          onPointerDown={(e) => start(c, e)}
          className={cn(
            "absolute z-10 cursor-grab rounded-[5px]",
            drag?.tok.n === c.tok.n && "bg-ground/60 cursor-grabbing",
          )}
          style={{ left: c.left, top: c.top, width: c.width, height: c.height }}
        />
      ))}
      {shown && img && (
        <Preview
          chip={shown}
          img={img}
          onEnter={keep}
          onLeave={hideSoon}
          onOpen={() => {
            keep();
            setHover(null);
            setLarge({ n: shown.tok.n, img });
          }}
        />
      )}
      <Dialog open={!!large} onOpenChange={(o) => !o && setLarge(null)}>
        <DialogContent
          className="flex max-h-[90vh] w-auto max-w-[90vw] items-center justify-center p-2"
          // Back to the box (the preview it was opened from is gone).
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            input.current?.focus();
          }}
        >
          <DialogTitle className="sr-only">图片预览</DialogTitle>
          <DialogDescription className="sr-only">图片 {large?.n}</DialogDescription>
          {large && (
            <img
              data-testid="image-large"
              src={large.img.url}
              alt=""
              className="max-h-[85vh] max-w-[88vw] object-contain"
            />
          )}
        </DialogContent>
      </Dialog>
      {drag?.line && (
        <span
          data-testid="image-drop"
          aria-hidden
          className="pointer-events-none absolute z-20 w-[2px] rounded-full bg-accent"
          style={{ left: drag.line.left - 1, top: drag.line.top, height: drag.line.height }}
        />
      )}
      {drag && (
        <span
          aria-hidden
          className="pointer-events-none fixed z-50 flex -rotate-2 items-center gap-1 rounded-[6px] border border-border bg-ground px-1.5 py-0.5 text-[12.5px] text-text shadow-[0_8px_20px_rgba(0,0,0,0.18),0_1px_3px_rgba(0,0,0,0.1)]"
          style={{ left: drag.x + 8, top: drag.y - 12 }}
        >
          <ImageIcon size={13} strokeWidth={1.8} className="text-secondary" aria-hidden />
          图片 {drag.tok.n}
        </span>
      )}
    </>
  );
}

const HIDE_DELAY = 200;

/** A hovered token's image, above it; the pointer can move onto it, a click on the image shows it large. */
function Preview({
  chip,
  img,
  onEnter,
  onLeave,
  onOpen,
}: {
  chip: ChipBox;
  img: DraftImage;
  onEnter: () => void;
  onLeave: () => void;
  onOpen: () => void;
}) {
  const [size, setSize] = useState<string | null>(null);
  return (
    <div
      role="tooltip"
      data-testid="image-preview"
      onPointerEnter={onEnter}
      onPointerLeave={onLeave}
      // Not a click into the box below: the caret stays where it is.
      onPointerDown={(e) => e.preventDefault()}
      className="absolute z-30 flex w-[280px] flex-col gap-1.5 rounded-[10px] border border-border bg-ground p-2 shadow-[0_12px_32px_rgba(0,0,0,0.16),0_1px_3px_rgba(0,0,0,0.08)]"
      style={{ left: Math.max(0, chip.left - 20), bottom: `calc(100% - ${chip.top}px + 6px)` }}
    >
      <button
        type="button"
        aria-label="放大图片"
        data-testid="image-preview-open"
        onClick={onOpen}
        className="cursor-zoom-in"
      >
        <img
          src={img.url}
          alt=""
          onLoad={(e) => setSize(`${e.currentTarget.naturalWidth}×${e.currentTarget.naturalHeight}`)}
          className="max-h-[164px] w-full rounded-md border border-border object-contain"
        />
      </button>
      <div className="flex justify-between text-[11px] text-secondary">
        <span>
          图片 {chip.tok.n}
          {size && ` · ${size}`}
        </span>
        <span>点图放大 · 拖动移位置 · ⌫ 删除</span>
      </div>
    </div>
  );
}
