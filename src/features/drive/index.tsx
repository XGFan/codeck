import { Plus, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/ipc";
import { formatCost, formatDuration, formatTokens } from "@/lib/format";
import { isComposingKey } from "@/lib/hotkeys";
import type { SessionQuery } from "@/ipc/bindings";
import { useSessionList, useSessions } from "@/queries";
import { useUi } from "@/state/ui";
import { Composer } from "./Composer";
import type { ConnState } from "./conn";
import type { QuestionEvent, TurnEndEvent } from "./protocol";
import { Requests } from "./Requests";
import { connFor, draftKey, errorKey, lsEntryFor, useDrive, waitingSessions } from "./store";

export { DriveConnect, STATE_WORD, useDriveDebugInfo } from "./Header";
export { QuestionForm } from "./Requests";
export { DriveSettings } from "./Settings";
export { connFor, lsEntryFor, useDrive } from "./store";

/**
 * The CoDeck's Live Sessions: from its `LiveChanged` events (each a full snapshot), and until the first one
 * arrives, from the Session list it already loaded (unfiltered at startup) — no query of its own.
 */
/** Every Session, unfiltered: the Live set until the first `LiveChanged` snapshot arrives. */
const ALL_SESSIONS: SessionQuery = { projectIds: [], sort: "lastActive", descending: true, timeRange: null, drill: null };

function useLive(): { id: string; waiting: boolean }[] {
  const live = useDrive((s) => s.live);
  // Not the list on screen: its project / time filter would hide Live Sessions elsewhere.
  const { data: list } = useSessions(ALL_SESSIONS, live == null);
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
  const conns = useDrive((s) => s.conns);
  return useMemo(() => waitingSessions(live.filter((l) => l.waiting).map((l) => l.id), conns), [live, conns]);
}

/**
 * Starts driving's background work (ccrpc's events; `ccrpc ls` polling while some Session is Live or driven)
 * and keeps the Dock badge. Mount once.
 */
export function DriveRoot() {
  const start = useDrive((s) => s.start);
  const setWanted = useDrive((s) => s.setWanted);
  useEffect(() => start(), [start]);
  const anyLive = useLive().length > 0;
  const anyConn = useDrive((s) => Object.keys(s.conns).length > 0);
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
      const ls = useDrive.getState().ls.find((e) => e.session_id === other);
      const title = list?.find((s) => s.id === other)?.title ?? ls?.title ?? ls?.cwd?.split("/").pop() ?? "另一个 Session";
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
      <button type="button" data-testid="elsewhere" onClick={() => select(shown.id)} className="min-w-0 flex-1 truncate text-left">
        {shown.title} 在等你作答，点此切过去
      </button>
      <button type="button" aria-label="关闭" onClick={() => setShown(null)}>
        <X size={12} aria-hidden />
      </button>
    </div>
  );
}

/** 「新建 Session」 at the top of the Session list: pick a folder, `ccrpc new`. */
export function NewSessionButton() {
  const newSession = useDrive((s) => s.newSession);
  return (
    <button
      type="button"
      data-testid="new-session"
      aria-label="新建 Session"
      title="新建 Session"
      onClick={() => void newSession()}
      className="flex size-[22px] shrink-0 items-center justify-center rounded-md text-secondary outline-none hover:bg-text/8 focus-visible:outline-2 focus-visible:outline-accent"
    >
      <Plus size={13} strokeWidth={1.8} aria-hidden />
    </button>
  );
}

/** Why the last 「新建 Session」 failed, under the Session list's title row; closable. */
export function NewSessionError() {
  const error = useDrive((s) => s.newError);
  const setNewError = useDrive((s) => s.setNewError);
  if (!error) return null;
  return (
    <div role="alert" data-testid="new-session-error" className="flex shrink-0 items-center gap-2 border-b border-border bg-error-bg px-3.5 py-1.5 text-[12px] text-error">
      <span className="min-w-0 flex-1">{error}</span>
      <button type="button" aria-label="关闭" onClick={() => setNewError(null)}>
        <X size={12} aria-hidden />
      </button>
    </div>
  );
}

/** The open Session's connection when it waits on this main-line AskUserQuestion (answered on its transcript card). */
export function useQuestionRequest(toolUseId: string): { r: QuestionEvent; c: ConnState } | null {
  const sessionId = useUi((s) => s.sessionId);
  const c = useDrive((s) => connFor(s, sessionId));
  const r = c?.requests.find((x): x is QuestionEvent => x.type === "question" && x.request_id === toolUseId && !x.agent_id);
  return c && r && !c.procExit && !c.sessionExited ? { r, c } : null;
}

/** A connected Session's turn ended and it has not been looked at since. */
export function useUnread(sessionId: string): boolean {
  return useDrive((s) => !!connFor(s, sessionId)?.unread);
}

/** The open Session is driven or in `ccrpc ls` but not in the index yet (a new one, before its first prompt). */
export function useUnindexed(error: unknown): boolean {
  const sessionId = useUi((s) => s.sessionId);
  const known = useDrive((s) => connFor(s, sessionId) != null || lsEntryFor(s, sessionId) != null);
  return known && !!error && typeof error === "object" && (error as { code?: unknown }).code === "notFound";
}

/** Under the transcript while the open Session is driven: banners, cards, progress, queue, input box. */
export function DrivePanel() {
  const sessionId = useUi((s) => s.sessionId);
  const c = useDrive((s) => connFor(s, sessionId));
  if (!c) return null;
  return <Panel key={draftKey(c)} c={c} />;
}

function Panel({ c }: { c: ConnState }) {
  const command = useDrive((s) => s.command);
  const reconnect = useDrive((s) => s.reconnect);
  const dismiss = useDrive((s) => s.dismissNotice);
  const setError = useDrive((s) => s.setError);
  const live = !c.procExit && !c.sessionExited && c.ready;

  // Esc: leaves a card's text field; aborts while busy / waiting; otherwise keeps CoDeck's meaning.
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "Escape" || isComposingKey(e.nativeEvent)) return;
    if (document.querySelector("[data-radix-popper-content-wrapper], [role=dialog]")) return;
    const el = e.target as HTMLElement;
    if (el.tagName === "INPUT") {
      e.preventDefault();
      e.stopPropagation();
      el.blur();
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
    <div data-testid="drive-panel" onKeyDown={onKeyDown} className="flex shrink-0 flex-col gap-2 border-t border-border px-6 pt-2.5 pb-3">
      <div className="mx-auto flex w-full max-w-[820px] flex-col gap-2">
        {c.procExit && (
          <div className="flex items-center gap-3 rounded-md bg-error-bg px-3 py-1.5 text-[12px] text-error" data-testid="proc-exit">
            <span className="min-w-0 flex-1 truncate" title={c.procExit.stderr}>
              ccrpc 已退出（{c.procExit.code == null ? "被信号终止" : `退出码 ${c.procExit.code}`}）{c.procExit.stderr ? `：${lastLine(c.procExit.stderr)}` : ""}
            </span>
            {c.holderId && (
              <button type="button" className="rounded border border-error px-2 py-0.5" onClick={() => void reconnect(c.conn)} data-testid="reconnect">
                重连
              </button>
            )}
          </div>
        )}
        {c.sessionExited && !c.procExit && (
          <div className="rounded-md bg-code px-3 py-1.5 text-[12px] text-secondary" data-testid="session-exit">
            Session 已结束
          </div>
        )}
        {c.notices.map((n) => (
          <div key={n.id} className="flex items-center gap-2 rounded-md bg-code px-3 py-1 text-[12px] text-secondary" data-testid="notice">
            <span className="min-w-0 flex-1 truncate">{n.text}</span>
            <button type="button" aria-label="关闭" onClick={() => dismiss(c.conn, n.id)}>
              <X size={12} aria-hidden />
            </button>
          </div>
        ))}
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

/**
 * The foot of the open Session's transcript while driven: Claude Code's progress line while busy (as in the
 * terminal, under the last message), else the last turn's end. Nothing when not driven.
 */
export function DriveTail() {
  const sessionId = useUi((s) => s.sessionId);
  const c = useDrive((s) => connFor(s, sessionId));
  if (!c || c.procExit) return null;
  if (c.state === "busy")
    return (
      <div className="flex items-center gap-2 pt-[18px] text-[13px] text-claude" data-testid="progress">
        <span className="size-3 shrink-0 animate-spin rounded-full border-2 border-claude/25 border-t-claude" aria-hidden />
        <span className="min-w-0 truncate">{c.progress?.text ?? "工作中…"}</span>
      </div>
    );
  return c.lastTurn ? <LastTurn ev={c.lastTurn} /> : null;
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
    <div data-testid="turn-end" data-prompt-id={ev.prompt_id ?? ""} className="flex flex-wrap items-center gap-x-3 pt-3 text-[11px] text-secondary">
      <span>上一轮</span>
      {parts.length > 0 && <span>{parts.join(" · ")}</span>}
      {ev.aborted && <span className="font-semibold">已中止</span>}
      {ev.error && <span className="text-error">{ev.error.message}</span>}
    </div>
  );
}

const lastLine = (s: string) => s.trim().split("\n").pop() ?? "";
