// The terminal window's page (`terminal.html?holder=<Holder ID>`): the Pane of a Connected Session, attached
// straight to its Holder. Typing, pasting and Esc go to Claude Code as they are; the Pane's size is never changed.

import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import "@xterm/xterm/css/xterm.css";
import { termBridge } from "./bridge";
import { areaFor, BASE_FONT, fontFor, type CellRatio } from "./fit";
import { parseMessage } from "./messages";
import type { TermStatus } from "./status";

const PAD_X = 12;
const PAD_Y = 8;
const BAR = 24;

const enc = new TextEncoder();

/** What ended the connection, or why it never started. */
type Ended = { text: string } | null;

export function TerminalPage({ holderId }: { holderId: string }) {
  const host = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ cols: number; rows: number } | null>(null);
  const [status, setStatus] = useState<TermStatus>("");
  const [ended, setEnded] = useState<Ended>(null);

  useEffect(() => {
    const el = host.current!;
    const term = new Terminal({
      allowProposedApi: true,
      fontFamily: 'Menlo, "SF Mono", ui-monospace, monospace',
      fontSize: BASE_FONT,
      scrollback: 5000,
      // Claude Code turns on mouse reporting: ⌥-drag still selects text to copy.
      macOptionClickForcesSelection: true,
      theme: {
        background: "#1f2023",
        foreground: "#e4e4e6",
        cursor: "#e4e4e6",
        selectionBackground: "rgba(76, 141, 246, 0.4)",
      },
    });
    term.loadAddon(new Unicode11Addon());
    // The width table Claude Code's layout and the Holder's VT agree with.
    term.unicode.activeVersion = "11";
    term.open(el);
    term.focus();

    // One cell per pixel of font size, measured from the rendered screen.
    const ratio = (): CellRatio | null => {
      const screen = el.querySelector<HTMLElement>(".xterm-screen");
      if (!screen || !screen.clientWidth) return null;
      const font = term.options.fontSize ?? BASE_FONT;
      return { width: screen.clientWidth / term.cols / font, height: screen.clientHeight / term.rows / font };
    };
    const fitFont = () => {
      const r = ratio();
      if (!r) return;
      const font = fontFor(term.cols, term.rows, el.clientWidth, el.clientHeight, r);
      if (font !== term.options.fontSize) term.options.fontSize = font;
    };
    const observer = new ResizeObserver(fitFont);
    observer.observe(el);

    const send = (bytes: Uint8Array) => void termBridge.input(bytes).catch(() => {});
    const typed = term.onData((d) => send(enc.encode(d)));
    // Mouse reports in the legacy encoding (bytes above 127).
    const binary = term.onBinary((b) => send(Uint8Array.from(b, (c) => c.charCodeAt(0) & 0xff)));
    // With an input method on (Pinyin…), WebKit gives Esc only a keyup and xterm never sees it: sent from the
    // keyup then, unless that Esc just ended a composition (closed the candidate list).
    let escDown = false;
    let composedAt = -Infinity;
    const composed = () => (composedAt = performance.now());
    // An Esc held while focus leaves: its keyup is not ours.
    const blurred = () => (escDown = false);
    term.textarea?.addEventListener("compositionend", composed);
    term.textarea?.addEventListener("blur", blurred);
    term.attachCustomKeyEventHandler((ev) => {
      if (ev.key !== "Escape") return true;
      if (ev.type === "keydown") escDown = true;
      if (ev.type === "keyup") {
        if (!escDown && !ev.isComposing && performance.now() - composedAt > 300) send(new Uint8Array([0x1b]));
        escDown = false;
      }
      return true;
    });

    let sized = false;
    const onMessage = (buf: ArrayBuffer | Uint8Array) => {
      const m = parseMessage(buf);
      if (!m) return;
      switch (m.kind) {
        case "size": {
          term.resize(m.cols, m.rows);
          setSize({ cols: m.cols, rows: m.rows });
          if (sized) {
            fitFont();
            break;
          }
          sized = true;
          // The window opens to fit the Pane at the base font (measured once the new size is drawn);
          // afterwards the font follows the window.
          term.options.fontSize = BASE_FONT;
          const drawn = term.onRender(() => {
            drawn.dispose();
            const r = ratio();
            if (!r) return;
            const area = areaFor(m.cols, m.rows, r);
            // Never larger than the screen; the font then shrinks to fit.
            const width = Math.min(area.width + 2 * PAD_X, window.screen.availWidth - 40);
            const height = Math.min(area.height + 2 * PAD_Y + BAR, window.screen.availHeight - 60);
            void termBridge
              .resizeWindow(width, height)
              .catch(() => {})
              .finally(fitFont);
          });
          break;
        }
        case "history":
          // Then pushed up into the scrollback, so the snapshot's clearing does not take its last screenful.
          term.write(m.data);
          term.write("\r\n".repeat(term.rows));
          break;
        case "snapshot":
        case "output":
          term.write(m.data);
          break;
        case "exit":
          setEnded({ text: "Pane 里的 shell 已退出" });
          break;
        case "closed":
          setEnded((e) => e ?? { text: "与 Pane 的连接已断开" });
          break;
      }
    };
    let unlisten: (() => void) | null = null;
    let gone = false;
    // A status heard while attaching is newer than the one `attach` returns.
    let heard = false;
    const onStatus = (s: TermStatus) => {
      heard = true;
      setStatus(s);
    };
    // Listening before attaching: a status set in between is either heard or returned by `attach`.
    void termBridge
      .onStatus(holderId, onStatus)
      .then((u) => {
        if (gone) return u();
        unlisten = u;
        return termBridge.attach(holderId, onMessage).then((s) => !heard && s != null && setStatus(s));
      })
      .catch((e: unknown) => setEnded({ text: String(e) }));

    // The window's own connection goes when the window does (the plugin detaches it).
    return () => {
      gone = true;
      unlisten?.();
      typed.dispose();
      binary.dispose();
      term.textarea?.removeEventListener("compositionend", composed);
      term.textarea?.removeEventListener("blur", blurred);
      observer.disconnect();
      term.dispose();
    };
  }, [holderId]);

  return (
    <div className="flex h-full flex-col bg-terminal text-[#e4e4e6]" data-testid="terminal-page">
      <div className="relative min-h-0 flex-1" style={{ padding: `${PAD_Y}px ${PAD_X}px` }}>
        <div ref={host} className="h-full w-full overflow-hidden" data-testid="terminal-screen" />
      </div>
      <div
        className="flex shrink-0 items-center gap-3 border-t border-[#38383b] px-3 text-[11.5px] text-[#a1a1a6]"
        style={{ height: BAR }}
        data-testid="terminal-bar"
      >
        {ended ? (
          <span className="text-[#f2705b]" data-testid="terminal-ended">
            {ended.text}
          </span>
        ) : (
          status === "input" && (
            <span className="flex items-center gap-1.5 text-[#5fc488]" data-testid="terminal-back">
              <span className="size-[7px] rounded-full bg-[#5fc488]" />
              Claude Code 已回到输入框
            </span>
          )
        )}
        {size && (
          <span data-testid="terminal-size">
            {size.cols}×{size.rows}
          </span>
        )}
        <span className="flex-1" />
        <span>按住 ⌥ 拖动选中文字</span>
      </div>
    </div>
  );
}
