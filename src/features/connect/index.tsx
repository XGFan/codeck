import { Info, Unplug, X } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api } from "@/ipc";
import { isNotFound } from "@/lib/errors";
import { formatCost, formatDuration, formatTokens } from "@/lib/format";
import { isComposingKey, popupOpen } from "@/lib/hotkeys";
import { useAllSessions, useSession, useSessionList } from "@/queries";
import { useUi } from "@/state/ui";
import { Composer } from "./Composer";
import type { ConnState } from "./conn";
import type { QuestionEvent, TurnEndEvent } from "./protocol";
import { ErrorNotice, NoticeCard } from "./Notice";
import { arrived, PENDING_MAX_MS } from "./pending";
import { Requests } from "./Requests";
import { resumeOffer } from "./resume";
import { connFor, draftKey, errorKey, isNewUnprompted, lsEntryFor, useConnect, waitingSessions } from "./store";

export { ConnectButton, ConnectMenuItems, STATE_WORD, useConnectDebugInfo } from "./Header";
export { arrived, isFresh } from "./pending";
export { hasPreview, OptionPreview } from "./OptionPreview";
export { QuestionForm } from "./QuestionForm";
export { RewindButton, RewoundDivider, useRewindView, useShownTranscript, type RewindView } from "./RewindViews";
export { ForkButton, ForkMenuItem, ForkPending } from "./ForkViews";
export { CcrpcSettings } from "./Settings";
export { connFor, isLive, lsEntryFor, useConnect } from "./store";

/**
 * The CoDeck's Live Sessions: from its `LiveChanged` events (each a full snapshot), and until the first one
 * arrives, from the Session list it already loaded (unfiltered at startup) — no query of its own.
 */
function useLive(): { id: string; waiting: boolean }[] {
  const live = useConnect((s) => s.live);
  // Not the list on screen: its project / time filter would hide Live Sessions elsewhere.
  const { data: list } = useAllSessions(live == null);
  return useMemo(
    () =>
      live
        ? live.map((e) => ({ id: e.sessionId, waiting: e.state.status === "waiting" }))
        : (list ?? []).filter((s) => s.live).map((s) => ({ id: s.id, waiting: s.live!.status === "waiting" })),
    [live, list],
  );
}

/** Sessions waiting for an answer: Live ones that wait (every Session), and connected ones with a card. */
function useWaiting(): string[] {
  const live = useLive();
  const conns = useConnect((s) => s.conns);
  return useMemo(
    () =>
      waitingSessions(
        live.filter((l) => l.waiting).map((l) => l.id),
        conns,
      ),
    [live, conns],
  );
}

/**
 * Starts Connect's background work (ccrpc's events; `ccrpc ls` polling while some Session is Live or connected)
 * and keeps the Dock badge. Mount once.
 */
export function ConnectRoot() {
  const start = useConnect((s) => s.start);
  const setWanted = useConnect((s) => s.setWanted);
  useEffect(() => start(), [start]);
  const anyLive = useLive().length > 0;
  const anyConn = useConnect((s) => Object.keys(s.conns).length > 0);
  useEffect(() => setWanted(anyLive || anyConn), [anyLive, anyConn, setWanted]);
  const n = useWaiting().length;
  useEffect(() => {
    void api.setBadge(n).catch(() => {});
  }, [n]);
  return null;
}

/**
 * On coming back to the window: the Session on screen has nothing to answer but another one waits — one
 * line at the top; clicking it opens that Session. Gone once the selection changes.
 */
export function ElsewhereBanner() {
  const waiting = useWaiting();
  const { data: list } = useSessionList();
  const ref = useRef({ waiting, list });
  ref.current = { waiting, list };
  const [shown, setShown] = useState<{ id: string; title: string } | null>(null);
  const select = useUi((s) => s.selectSession);
  useEffect(() => {
    const check = () => {
      const cur = useUi.getState().sessionId;
      const { waiting, list } = ref.current;
      const other = waiting.find((id) => id !== cur);
      if ((cur != null && waiting.includes(cur)) || !other) return setShown(null);
      const ls = useConnect.getState().ls.find((e) => e.session_id === other);
      const title =
        list?.find((s) => s.id === other)?.title ?? ls?.title ?? ls?.cwd?.split("/").pop() ?? "另一个 Session";
      setShown({ id: other, title });
    };
    window.addEventListener("focus", check);
    const un = useUi.subscribe((s, p) => {
      if (s.sessionId !== p.sessionId) setShown(null);
    });
    return () => {
      window.removeEventListener("focus", check);
      un();
    };
  }, []);
  if (!shown) return null;
  return (
    <div className="flex shrink-0 items-center gap-2 bg-selection px-6 py-1.5 text-[12px] text-accent">
      <button
        type="button"
        data-testid="elsewhere"
        onClick={() => select(shown.id)}
        className="min-w-0 flex-1 truncate text-left"
      >
        {shown.title} 在等你作答，点此切过去
      </button>
      <button type="button" aria-label="关闭" onClick={() => setShown(null)}>
        <X size={12} aria-hidden />
      </button>
    </div>
  );
}

/** Why the last 「新建 Session」 failed, under the Session list's title row; closable. */
export function NewSessionError() {
  const error = useConnect((s) => s.newError);
  const setNewError = useConnect((s) => s.setNewError);
  if (!error) return null;
  return (
    <div
      role="alert"
      data-testid="new-session-error"
      className="flex shrink-0 items-center gap-2 border-b border-border bg-error-bg px-3.5 py-1.5 text-[12px] text-error"
    >
      {/* In the sidebar, where text is not selectable; an error is worth copying. */}
      <span className="min-w-0 flex-1 select-text">{error}</span>
      <button type="button" aria-label="关闭" onClick={() => setNewError(null)}>
        <X size={12} aria-hidden />
      </button>
    </div>
  );
}

/** The open Session's connection when it waits on this main-line AskUserQuestion (answered on its transcript card). */
export function useQuestionRequest(toolUseId: string): { r: QuestionEvent; c: ConnState } | null {
  const sessionId = useUi((s) => s.sessionId);
  const c = useConnect((s) => connFor(s, sessionId));
  const r = c?.requests.find(
    (x): x is QuestionEvent => x.type === "question" && x.request_id === toolUseId && !x.agent_id,
  );
  return c && r && !c.procExit && !c.sessionExited ? { r, c } : null;
}

/** A connected Session's turn ended and it has not been looked at since. */
export function useUnread(sessionId: string): boolean {
  return useConnect((s) => !!connFor(s, sessionId)?.unread);
}

/** The open Session is connected or in `ccrpc ls` but not in the index yet (a new one, before its first prompt). */
export function useUnindexed(error: unknown): boolean {
  const sessionId = useUi((s) => s.sessionId);
  const known = useConnect((s) => connFor(s, sessionId) != null || lsEntryFor(s, sessionId) != null);
  // 「新建 Session」 before its first prompt has no transcript file: known without asking the index (no blank page
  // while the page moves from the connection's id to the Session's).
  const fresh = useConnect((s) => {
    const c = connFor(s, sessionId);
    return !!c && isNewUnprompted(c);
  });
  return fresh || (known && isNotFound(error));
}

/** Under the transcript while the open Session is connected: banners, cards, progress, queue, input box. */
export function ConnectPanel() {
  const sessionId = useUi((s) => s.sessionId);
  const c = useConnect((s) => connFor(s, sessionId));
  if (!c) return null;
  // A new Session's panel stays as its id comes (typing goes on); a later Session of the connection gets its own.
  return <Panel key={c.newIn != null ? c.conn : draftKey(c)} c={c} />;
}

function Panel({ c }: { c: ConnState }) {
  const command = useConnect((s) => s.command);
  const answerRewind = useConnect((s) => s.answerRewind);
  const reconnect = useConnect((s) => s.reconnect);
  const dismiss = useConnect((s) => s.dismissNotice);
  const setError = useConnect((s) => s.setError);
  const live = !c.procExit && !c.sessionExited && c.ready;
  const storeError = useConnect((s) => s.errors[errorKey(c)]);
  const floatRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = floatRef.current;
    const host = el?.parentElement;
    if (!el || !host) return;
    const ro = new ResizeObserver(() => host.style.setProperty("--connect-h", `${el.offsetHeight}px`));
    ro.observe(el);
    return () => {
      ro.disconnect();
      host.style.removeProperty("--connect-h");
    };
  }, []);

  // Esc: leaves a card's text field; aborts while busy / waiting; otherwise keeps CoDeck's meaning.
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "Escape" || isComposingKey(e.nativeEvent)) return;
    if (popupOpen()) return;
    const el = e.target as HTMLElement;
    if (el.tagName === "INPUT") {
      e.preventDefault();
      e.stopPropagation();
      el.blur();
      return;
    }
    // While ccrpc is in the terminal's Rewind picker nothing interrupts it; on the rewind's confirm page Esc cancels
    // it (an abort would only step back to the picker).
    const rw = live ? c.requests.find((r) => r.type === "rewind") : undefined;
    if (live && (c.rewinding || rw)) {
      e.preventDefault();
      e.stopPropagation();
      if (rw && !c.rewinding) void answerRewind(c.conn, rw, null);
      return;
    }
    if (!live || (c.state !== "busy" && c.state !== "waiting")) return;
    e.preventDefault();
    e.stopPropagation();
    void command(c.conn, { type: "abort" }).then((r) => {
      if (!r.success) setError(errorKey(c), `中止失败：${r.error?.message ?? ""}`);
    });
  };

  return (
    // Floats over the foot of the transcript (Codex style): the transcript scrolls under it and keeps room for it
    // at its end (`--connect-h`, set on the shared parent).
    <div
      ref={floatRef}
      data-testid="connect-panel"
      data-connect-panel
      data-conn={c.conn}
      onKeyDown={onKeyDown}
      className="pointer-events-none absolute right-3 bottom-0 left-0 flex flex-col bg-gradient-to-b from-transparent via-ground/95 via-30% to-ground px-6 pt-8 pb-3"
    >
      <div className="pointer-events-auto mx-auto flex w-full max-w-(--conv-w) flex-col gap-2">
        {c.procExit && (
          <NoticeCard
            tone="error"
            testId="proc-exit"
            icon={<Unplug size={18} strokeWidth={1.7} />}
            title={`ccrpc 已退出（${c.procExit.code == null ? "被信号终止" : `退出码 ${c.procExit.code}`}）`}
            desc={c.procExit.stderr ? lastLine(c.procExit.stderr) : undefined}
            action={
              c.holderId
                ? { label: "重连", primary: true, onClick: () => void reconnect(c.conn), testId: "reconnect" }
                : undefined
            }
          />
        )}
        {c.sessionExited && !c.procExit && <SessionExited c={c} />}
        {c.notices.map((n) => (
          <NoticeCard
            key={n.id}
            testId="notice"
            icon={<Info size={18} strokeWidth={1.7} />}
            title={n.text}
            onClose={() => dismiss(c.conn, n.id)}
          />
        ))}
        {storeError && <ErrorNotice text={storeError} onClose={() => setError(errorKey(c), null)} />}
        {/* Ended (ccrpc exited, or the Session did): only the line above says so; nothing to type into. */}
        {!c.procExit && !c.sessionExited && (
          <>
            <Requests c={c} />
            <Composer c={c} />
          </>
        )}
      </div>
    </div>
  );
}

/** The Connected Session ended (`session_exit`): only this card, with 「Resume」 when the Session can be. */
function SessionExited({ c }: { c: ConnState }) {
  const sessionId = c.sessionId ?? c.resumeOf;
  const { data: d } = useSession(sessionId);
  const startResume = useConnect((s) => s.resume);
  const ccrpcError = useConnect((s) => s.lsError ?? s.infoError);
  // Back in clay already (resumed in the terminal): 「Connect」 in the header's menu, not a second claude.
  const inLs = useConnect((s) => lsEntryFor(s, sessionId) != null);
  const busy = useConnect((s) => (sessionId ? !!s.resuming[sessionId] : false));
  const offer =
    d && sessionId
      ? resumeOffer({ live: d.summary.live != null || inLs, cwd: d.cwd, cwdMissing: d.cwdMissing, ccrpcError })
      : null;
  return (
    <NoticeCard
      testId="session-exit"
      icon={<Unplug size={18} strokeWidth={1.7} />}
      title="Session 已结束"
      action={
        offer && offer.kind !== "hidden" && sessionId && d?.cwd
          ? {
              label: "Resume",
              primary: true,
              disabled: offer.kind === "disabled" || busy,
              title:
                offer.kind === "disabled"
                  ? offer.why
                  : "在新的 clay Pane 里 claude --resume，从 Main Line 末尾继续，然后 Connect",
              testId: "session-exit-resume",
              onClick: () => void startResume(sessionId, d.cwd!),
            }
          : undefined
      }
    />
  );
}

/**
 * The foot of the open Session's transcript while connected: the last turn's end, a prompt just sent (a placeholder
 * until its node is in `promptIds`, the prompts shown), and Claude Code's progress line while busy (as in the
 * terminal, under the last message). Nothing when not connected.
 */
export function ConnectTail({ promptIds }: { promptIds?: ReadonlySet<string> }) {
  const sessionId = useUi((s) => s.sessionId);
  const c = useConnect((s) => connFor(s, sessionId));
  if (!c || c.procExit) return null;
  return (
    <>
      {c.state !== "busy" && c.lastTurn && <LastTurn ev={c.lastTurn} />}
      {/* Not under a new Session's empty transcript (its first prompt shows with the transcript). */}
      {c.pending && promptIds && <PendingPrompt c={c} promptIds={promptIds} />}
      {c.state === "busy" && <Progress c={c} />}
    </>
  );
}

/**
 * A prompt sent, before its node is in the transcript: a bubble of bouncing dots where it will be (not its text: the
 * transcript draws it as indexed). It goes once the node shows (which plays its arrival), or after `PENDING_MAX_MS`.
 */
function PendingPrompt({ c, promptIds }: { c: ConnState; promptIds: ReadonlySet<string> }) {
  const setPending = useConnect((s) => s.setPending);
  const p = c.pending!;
  const shown = !!p.uuid && promptIds.has(p.uuid);
  const conn = c.conn;
  useEffect(() => {
    if (!shown) return;
    void api.ccrpcGuiLog(conn, `prompt shown ${Date.now() - p.at}ms after send`).catch(() => {});
    setPending(conn, null);
  }, [shown, conn, p.at, setPending]);
  useEffect(() => {
    if (!p.uuid) return;
    const uuid = p.uuid;
    const t = window.setTimeout(() => {
      arrived(uuid);
      setPending(conn, null);
    }, PENDING_MAX_MS);
    return () => window.clearTimeout(t);
  }, [p.uuid, conn, setPending]);
  if (shown) return null;
  return (
    <div data-testid="pending-prompt" className="flex flex-col items-end gap-1 pt-[18px]">
      <div
        className="pending-in flex h-[38px] items-center gap-1 rounded-2xl bg-selection/60 px-3.5"
        aria-label="发送中"
      >
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="typing-dot size-1.5 rounded-full bg-secondary"
            style={{ animationDelay: `${i * 160}ms` }}
            aria-hidden
          />
        ))}
      </div>
      {/* The meta line's room (time, copy) so nothing moves when the bubble arrives. */}
      <div className="h-5" aria-hidden />
    </div>
  );
}

function Progress({ c }: { c: ConnState }) {
  return (
    // Grey with a passing highlight (as in Codex): Claude's clay reads as an error here.
    <div className="flex items-center gap-2 pt-[18px] text-[13px] text-secondary" data-testid="progress">
      <span
        className="size-3 shrink-0 animate-spin rounded-full border-2 border-secondary/25 border-t-secondary"
        aria-hidden
      />
      <span className="shimmer-text min-w-0 truncate">{c.progress?.text ?? "工作中…"}</span>
    </div>
  );
}

/** The last turn's end: duration, tokens, estimated cost; error; aborted. */
function LastTurn({ ev }: { ev: TurnEndEvent }) {
  const tokens = ev.usage.input_tokens + ev.usage.output_tokens;
  const parts = [
    ev.duration_ms != null ? `用时 ${formatDuration(ev.duration_ms)}` : null,
    tokens > 0 ? `${formatTokens(tokens)} token` : null,
    ev.api_cost != null ? `Cost ${formatCost(ev.api_cost)}` : null,
  ].filter(Boolean);
  return (
    <div
      data-testid="turn-end"
      data-prompt-id={ev.prompt_id ?? ""}
      className="flex flex-wrap items-center gap-x-3 pt-3 text-[11px] text-secondary"
    >
      <span>上一轮</span>
      {parts.length > 0 && <span>{parts.join(" · ")}</span>}
      {ev.aborted && <span className="font-semibold">已中止</span>}
      {ev.error && <span className="text-error">{ev.error.message}</span>}
    </div>
  );
}

const lastLine = (s: string) => s.trim().split("\n").pop() ?? "";
