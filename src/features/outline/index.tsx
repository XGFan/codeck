import { useHotkey } from "@/lib/hotkeys";
import { cn } from "@/lib/cn";
import { useUi } from "@/state/ui";
import { setCurrentTurn, type TurnInfo, useReading } from "../transcript/reading";
import { scrollToNode } from "../transcript/Transcript";

const main = () => document.querySelector<HTMLElement>('[data-testid="transcript"]');

function goTo(t: TurnInfo) {
  setCurrentTurn(t.turn, true);
  scrollToNode({ scope: { kind: "main" }, nodeId: t.nodeId, flashMs: 1200 });
}

/** `j` / `k` turn navigation and ⌘↑ / ⌘↓ (top / bottom); inactive while typing in an input or with ⌘K / Settings open. */
function useTurnKeys() {
  const step = (d: 1 | -1) => {
    const ui = useUi.getState();
    if (ui.searchOpen || ui.statsOpen || ui.settingsOpen) return;
    const { turns, current } = useReading.getState();
    const t = d === 1 ? turns.find((x) => x.turn > current) : [...turns].reverse().find((x) => x.turn < current);
    if (t) goTo(t);
  };
  useHotkey("j", () => step(1));
  useHotkey("k", () => step(-1));
  useHotkey("Meta+ArrowUp", () => main()?.scrollTo({ top: 0 }));
  useHotkey("Meta+ArrowDown", () => {
    const el = main();
    el?.scrollTo({ top: el.scrollHeight });
  });
}

/**
 * 轮次大纲: one tick per turn, floating over the left margin of the conversation (it takes no width), centered
 * between the header and the input area; the current turn is blue, hover lengthens a tick and shows the prompt.
 * Rendered in the transcript's positioned parent, where `--connect-h` is the floating input area's height.
 * Only the ticks take the pointer, each 16px wide: in a narrow pane the conversation column's left width handle
 * (8–20px) stays draggable beside and above / below them. Many turns shrink the ticks to 2px apart; past that the
 * list runs off the bottom (safe centering), never over the header.
 */
export function Outline() {
  const sessionId = useUi((s) => s.sessionId);
  const turns = useReading((s) => s.turns);
  const current = useReading((s) => s.current);
  useTurnKeys();
  if (!sessionId || turns.length === 0) return null;
  return (
    <nav
      aria-label="轮次大纲"
      data-testid="outline"
      className="pointer-events-none absolute top-0 left-0 z-20 flex w-6 flex-col justify-[safe_center] py-6"
      style={{ bottom: "var(--connect-h, 0px)" }}
    >
      {turns.map((t) => {
        const on = t.turn === current;
        return (
          <button
            key={t.nodeId}
            type="button"
            data-testid="outline-tick"
            data-turn={t.turn}
            data-current={on || undefined}
            aria-label={`第 ${t.turn + 1} 轮：${t.text.slice(0, 30)}`}
            aria-current={on || undefined}
            onClick={() => goTo(t)}
            className="group pointer-events-auto relative flex max-h-2 min-h-0.5 w-4 flex-1 basis-0 items-center pl-1.5"
          >
            <span
              className={cn(
                "block h-[2px] rounded-full transition-all",
                on ? "w-2.5 bg-accent" : "w-2 bg-secondary/50 group-hover:w-2.5 group-hover:bg-text",
              )}
            />
            <span
              role="tooltip"
              data-testid="outline-tip"
              className="pointer-events-none absolute top-1/2 left-full z-30 ml-1 hidden w-max max-w-64 -translate-y-1/2 rounded-md border border-border bg-ground px-2 py-1 text-left text-[12px] leading-snug whitespace-normal text-text shadow-lg group-hover:block group-focus-visible:block"
            >
              {t.text.length > 30 ? `${t.text.slice(0, 30)}…` : t.text}
            </span>
          </button>
        );
      })}
    </nav>
  );
}
