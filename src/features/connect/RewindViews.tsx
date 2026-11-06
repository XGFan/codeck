// Rewind's parts outside the input area's cards: 「回退」 on the transcript's prompts, the marker over the rewound
// part, and the 「选回退点」 notice.
import { RotateCcw } from "lucide-react";
import { useEffect, useMemo } from "react";
import type { Transcript } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { isComposingKey, popupOpen } from "@/lib/hotkeys";
import { scrollToNode } from "@/features/transcript/nodes/scroll";
import type { Row } from "@/features/transcript/grouping";
import { MAIN_SCOPE, selectOverlayOpen, useUi } from "@/state/ui";
import type { ConnState } from "./conn";
import { NoticeCard } from "./Notice";
import { mainLinePrompts, rewindable, rewoundLabel } from "./rewind";
import { connFor, isLive, useConnect } from "./store";

export interface RewindView {
  /** Prompts offering 「回退」. */
  rewindable: ReadonlySet<string>;
  /** Prompts offering 「Fork」: the Main Line's but the first, Connected or not. */
  forkable: ReadonlySet<string>;
  /** The rewound prompt's row: from it on the transcript is dimmed, under the divider `label`. */
  dimFrom: number | null;
  label: string;
}

/** The connection of the Session `sessionId` while it can be driven. */
const liveConn = (s: Parameters<typeof connFor>[0], sessionId: string | null) => {
  const c = connFor(s, sessionId);
  return c && isLive(c) && c.ready ? c : null;
};

/**
 * Rewind and Fork in the main transcript (`transcript` null elsewhere): the Main Line prompts offering 「Fork」, and while
 * Connected those offering 「回退」 and the rewound part. Entering 「选回退点」 scrolls to the last prompt; the marker goes once its prompt is no
 * longer on the indexed Main Line (the next prompt forked).
 */
export function useRewindView(transcript: Transcript | null, rows: Row[]): RewindView | null {
  const sessionId = transcript?.sessionId ?? null;
  const conn = useConnect((s) => liveConn(s, sessionId)?.conn ?? null);
  const rewound = useConnect((s) => liveConn(s, sessionId)?.rewound ?? null);
  const picking = useConnect((s) => !!liveConn(s, sessionId)?.picking);
  const clearRewound = useConnect((s) => s.clearRewound);
  const main = useMemo(
    () => (transcript ? mainLinePrompts(transcript.nodes, transcript.branchPoints) : null),
    [transcript],
  );
  const view = useMemo<RewindView | null>(() => {
    if (!main) return null;
    const ids = conn ? rewindable(main.ids, rewound?.uuid ?? null) : [];
    // Shown only on the Main Line itself: on another Branch, what follows the prompt is not what was rewound.
    const at =
      rewound && main.wholeMain ? rows.findIndex((r) => r.kind === "prompt" && r.node.id === rewound.uuid) : -1;
    return {
      rewindable: new Set(ids),
      forkable: new Set(main.ids.slice(1)),
      dimFrom: at >= 0 ? at : null,
      label: rewound ? rewoundLabel(rewound.withCode) : "",
    };
  }, [main, conn, rewound, rows]);
  const gone = !!main?.wholeMain && !!rewound && !main.ids.includes(rewound.uuid);
  useEffect(() => {
    if (gone && conn) clearRewound(conn);
  }, [gone, conn, clearRewound]);
  const last = view ? [...view.rewindable].pop() : undefined;
  useEffect(() => {
    if (picking && last) scrollToNode({ scope: MAIN_SCOPE, nodeId: last });
    // Only on entering: the list growing meanwhile does not scroll again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [picking]);
  return view;
}

/** 「回退」 under a Main Line prompt: on hover, always while picking; only while the Session is idle. */
export function RewindButton({ nodeId }: { nodeId: string }) {
  const sessionId = useUi((s) => s.sessionId);
  const c = useConnect((s) => liveConn(s, sessionId));
  const rewind = useConnect((s) => s.rewind);
  if (!c) return null;
  const running = c.rewinding?.uuid === nodeId;
  const idle = c.state === "idle" && c.rewinding == null;
  return (
    <button
      type="button"
      data-testid="rewind-button"
      data-picking={c.picking || undefined}
      disabled={!idle}
      title={idle ? "回退到这条提示词之前" : "空闲时才能回退"}
      onClick={(e) => {
        e.stopPropagation();
        void rewind(c.conn, nodeId);
      }}
      className={cn(
        "flex h-5 items-center gap-1 rounded px-1.5 text-[11px] transition-opacity disabled:cursor-default",
        c.picking || running
          ? "bg-selection text-accent"
          : "text-secondary opacity-0 group-focus-within/copy:opacity-100 group-hover/copy:opacity-100 hover:bg-selection hover:text-text focus-visible:opacity-100 disabled:hover:bg-transparent disabled:hover:text-secondary",
      )}
    >
      <RotateCcw size={11} strokeWidth={1.8} aria-hidden />
      {running ? "回退中…" : "回退"}
    </button>
  );
}

/** The line above the rewound prompt. */
export function RewoundDivider({ label }: { label: string }) {
  return (
    <div data-testid="rewound-divider" className="mb-3 flex items-center gap-2 text-[12.5px] text-accent">
      <RotateCcw size={13} strokeWidth={1.9} className="shrink-0" aria-hidden />
      <span className="min-w-0">{label}</span>
      <span className="h-px flex-1 bg-accent-border" />
    </div>
  );
}

/**
 * 「选回退点」: above the input box; 「取消」 leaves it, and so does Esc unless it closes something else first (a menu,
 * search, ⌘F, the Subagent panel; the slash command list).
 */
export function RewindPickNotice({ c }: { c: ConnState }) {
  const setPicking = useConnect((s) => s.setPicking);
  const conn = c.conn;
  const leave = () => setPicking(conn, false);
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      // Taken already: a handler before this one closed what it was for.
      if (e.key !== "Escape" || e.defaultPrevented || isComposingKey(e)) return;
      const ui = useUi.getState();
      if (popupOpen() || selectOverlayOpen(ui) || ui.findOpen || ui.panelStack.length > 0) return;
      e.preventDefault();
      setPicking(conn, false);
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [conn, setPicking]);
  return (
    <NoticeCard
      testId="rewind-pick"
      icon={<RotateCcw size={18} strokeWidth={1.7} />}
      title="选一条提示词，回退到它之前"
      desc="点提示词下方的「回退」 · Esc 取消"
      action={{ label: "取消", onClick: leave, testId: "rewind-pick-cancel" }}
    />
  );
}
