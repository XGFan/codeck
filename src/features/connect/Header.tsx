import { ChevronDown, Power, RotateCcw, ShieldCheck, ShipWheel, Unlink } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { api } from "@/ipc";
import { cn } from "@/lib/cn";
import { formatTokens } from "@/lib/format";
import type { ConnState } from "./conn";
import { ModelPicker } from "./Model";
import { resumeOffer } from "./resume";
import { connFor, errorKey, holderConnected, isLive, lsEntryFor, useConnect } from "./store";

const MODES: { id: string; label: string }[] = [
  { id: "manual", label: "手动审批" },
  { id: "accept_edits", label: "自动接受编辑" },
  { id: "plan", label: "计划模式" },
  { id: "auto", label: "自动模式" },
];

/** The one word for a Session's state (header, list dots): the connection's while connected, else the Live one. */
export const STATE_WORD: Record<string, string> = {
  starting: "启动中",
  idle: "空闲",
  busy: "工作中",
  waiting: "等待作答",
  exited: "已结束",
};

const pill = "flex h-7 items-center gap-1.5 rounded-md border border-border bg-ground px-2.5 text-[12px] hover:bg-selection disabled:opacity-50 disabled:hover:bg-ground";

/** A quiet button in the input box's foot. */
export const footBtn = "flex h-7 items-center gap-1.5 rounded-lg px-2 hover:bg-selection/60 hover:text-text disabled:opacity-50 disabled:hover:bg-transparent";

/**
 * 「Connect」 in the header's title row, for a Session in `ccrpc ls` that is not connected (disabled until its ccrpc
 * plugin has reported), or whose connection ended with the Session (connecting again replaces it). 「Disconnect」 and
 * 「关闭」 in its place while connected; the other controls are at the foot of the input box ([`ConnectFooter`]).
 * An ended Session not in clay gets 「Resume」 instead (`resume`: what the index knows of it).
 */
export function ConnectButton({ sessionId, resume }: { sessionId: string; resume?: { cwd: string | null; cwdMissing: boolean; live: boolean } }) {
  const entry = useConnect((s) => lsEntryFor(s, sessionId));
  const c = useConnect((s) => connFor(s, sessionId));
  const taken = useConnect((s) => (entry ? holderConnected(s, entry.holder_id) : false));
  const connect = useConnect((s) => s.connect);
  const disconnect = useConnect((s) => s.disconnect);
  const error = useConnect((s) => s.errors[sessionId]);
  const ccrpcError = useConnect((s) => s.lsError ?? s.infoError);
  const startResume = useConnect((s) => s.resume);
  const resuming = useConnect((s) => !!s.resuming[sessionId]);
  const [connecting, setConnecting] = useState(false);

  // A Resume on its way: until ccrpc names the Session, or its failure is dealt with.
  if (resuming)
    return (
      <span data-testid="resume-pending" className={cn(pill, "opacity-60")}>
        <RotateCcw size={13} strokeWidth={1.6} aria-hidden />
        Resume 中…
      </span>
    );
  // Connected: 「Disconnect」 in the same place (closes ccrpc's stdin; the Session keeps running), and 「关闭」 beside it.
  if (c && isLive(c))
    return (
      <>
        <button type="button" data-testid="connect-disconnect" onClick={() => void disconnect(c.conn)} className={pill} title="Disconnect 后 Session 照常运行">
          <Unlink size={13} strokeWidth={1.6} aria-hidden />
          Disconnect
        </button>
        <ConnectClose c={c} />
      </>
    );
  // Not in clay: an ended Session can be resumed (not one running elsewhere).
  if (!entry) {
    const offer = resume ? resumeOffer({ ...resume, ccrpcError }) : ({ kind: "hidden" } as const);
    if (offer.kind === "hidden") return error ? <ConnectError errKey={sessionId} text={error} /> : null;
    return (
      <>
        {error && <ConnectError errKey={sessionId} text={error} />}
        <button
          type="button"
          data-testid="resume-button"
          disabled={offer.kind === "disabled"}
          title={offer.kind === "disabled" ? offer.why : "在新的 clay Pane 里 claude --resume，从 Main Line 末尾继续，然后 Connect"}
          onClick={() => resume?.cwd && void startResume(sessionId, resume.cwd)}
          className={pill}
        >
          <RotateCcw size={13} strokeWidth={1.6} aria-hidden />
          Resume
        </button>
      </>
    );
  }
  // In clay but its Holder is connected for another Session (after `/clear` there).
  if (taken) return error ? <ConnectError errKey={sessionId} text={error} /> : null;
  return (
    <>
      {error && <ConnectError errKey={sessionId} text={error} />}
      <button
        type="button"
        data-testid="connect-button"
        disabled={!entry.plugin || connecting}
        title={entry.plugin ? "经 ccrpc Connect，在这里发提示词、作答审批" : "ccrpc 插件未加载，重启 claude 后可 Connect"}
        onClick={async () => {
          setConnecting(true);
          await connect(entry.holder_id, sessionId);
          setConnecting(false);
        }}
        className={pill}
      >
        <ShipWheel size={13} strokeWidth={1.6} aria-hidden />
        Connect
      </button>
    </>
  );
}

/**
 * 「关闭」: disconnects and ends the Session with its Pane (ccrpc `exit`, aborting a running turn first). A running
 * turn or background task is ended only after a confirmation.
 */
function ConnectClose({ c }: { c: ConnState }) {
  const close = useConnect((s) => s.close);
  const [confirming, setConfirming] = useState(false);
  const [closing, setClosing] = useState(false);
  const busy = c.state === "busy" || c.state === "waiting";
  const running = c.tasks.filter((t) => t.status === "running").length;
  const go = async () => {
    setConfirming(false);
    setClosing(true);
    await close(c.conn);
    setClosing(false);
  };
  const ends = [busy && "中止本轮", running > 0 && `停掉 ${running} 个后台任务`].filter(Boolean).join("、");
  return (
    <>
      <button
        type="button"
        data-testid="connect-close"
        disabled={closing || !(busy || c.state === "idle")}
        onClick={() => (busy || running > 0 ? setConfirming(true) : void go())}
        className={pill}
        title="Disconnect，并结束 Session、关闭它的 Pane"
      >
        <Power size={13} strokeWidth={1.6} aria-hidden />
        {closing ? "关闭中…" : "关闭"}
      </button>
      <Dialog open={confirming} onOpenChange={setConfirming}>
        <DialogContent data-testid="connect-close-confirm" className="w-[400px]">
          <DialogTitle>关闭 Session？</DialogTitle>
          <DialogDescription className="text-[13px] text-secondary">将{ends}，结束 Session 并关闭它的 Pane。</DialogDescription>
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="outline" onClick={() => setConfirming(false)}>
              取消
            </Button>
            <Button data-testid="connect-close-ok" onClick={() => void go()}>
              关闭
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}

/** A connect error, in the header (there is no input box yet); a click closes it. */
function ConnectError({ errKey, text }: { errKey: string; text: string }) {
  const setError = useConnect((s) => s.setError);
  return (
    <button type="button" data-testid="connect-error" title="关闭" onClick={() => setError(errKey, null)} className="max-w-72 truncate text-[12px] text-error">
      {text}
    </button>
  );
}

/** Context window size for the ring: 1M for the `[1m]` models, else 200K. */
const windowOf = (model: string | null) => (model && /\[1m\]|1m/i.test(model) ? 1_000_000 : 200_000);

/** How full the context is, as a small ring (Codex style). */
function ContextRing({ tokens, model }: { tokens: number; model: string | null }) {
  const f = Math.min(1, tokens / windowOf(model));
  const c = 2 * Math.PI * 9;
  return (
    <span className="flex shrink-0 items-center gap-1.5 text-secondary" title={`上下文 ${formatTokens(tokens)} / ${formatTokens(windowOf(model))}`} data-testid="connect-context">
      <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden>
        <circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" strokeOpacity="0.2" strokeWidth="3" />
        <circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" strokeWidth="3" strokeDasharray={`${f * c} ${c}`} transform="rotate(-90 12 12)" />
      </svg>
      {formatTokens(tokens)}
    </span>
  );
}

/**
 * The foot of the input box (Codex style): the permission mode at the left (`set_mode`; idle or busy; 自动模式
 * stands out), then `children` (pasted images, background tasks); context use and the model at the right, before
 * the send button. Errors show above the box, the connection state in the header.
 */
export function ConnectFooter({ c, children }: { c: ConnState; children?: React.ReactNode }) {
  const command = useConnect((s) => s.command);
  const setError = useConnect((s) => s.setError);
  const live = isLive(c);
  const mode = MODES.find((m) => m.id === c.mode);
  const setMode = async (id: string) => {
    const r = await command(c.conn, { type: "set_mode", mode: id });
    if (!r.success) setError(errorKey(c), `切换权限模式失败：${r.error?.message ?? ""}`);
  };
  return (
    <div data-testid="connect-controls" className="flex min-w-0 flex-1 items-center gap-1 text-[12.5px] text-secondary">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            data-testid="connect-mode"
            disabled={!live || !(c.state === "idle" || c.state === "busy")}
            className={cn(footBtn, c.mode === "auto" && "text-live-waiting")}
          >
            <ShieldCheck size={14} strokeWidth={1.7} aria-hidden />
            {mode?.label ?? c.mode ?? "权限模式"}
            <ChevronDown size={11} className="opacity-60" aria-hidden />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          {MODES.map((m) => (
            <DropdownMenuItem key={m.id} onSelect={() => void setMode(m.id)} className={cn(m.id === c.mode && "font-semibold")}>
              {m.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      {children}
      <span className="flex-1" />
      {c.contextTokens != null && <ContextRing tokens={c.contextTokens} model={c.model} />}
      <ModelPicker c={c} />
    </div>
  );
}

/** What to paste when reporting a connection problem: ids, cwd and the connection's log directory; null when not connected. */
export function useConnectDebugInfo(sessionId: string | null): string | null {
  const c = useConnect((s) => connFor(s, sessionId));
  const entry = useConnect((s) => lsEntryFor(s, sessionId));
  const logDir = useLogDir(c);
  if (!c || c.procExit) return null;
  const cwd = c.cwd ?? entry?.cwd ?? null;
  return [`holder_id: ${c.holderId ?? "-"}`, `session_id: ${c.sessionId ?? "-"}`, `cwd: ${cwd ?? "-"}`, `logs: ${logDir ?? "-"}`].join("\n");
}

/** The connection's log directory; asked again once a new Session's directory is renamed after `ready`. */
function useLogDir(c: ConnState | null): string | null {
  const [dir, setDir] = useState<string | null>(null);
  const conn = c?.conn;
  const holder = c?.holderId;
  useEffect(() => {
    if (!conn) return;
    let gone = false;
    void api.ccrpcLogDir(conn).then(
      (d) => !gone && setDir(d),
      () => {},
    );
    return () => {
      gone = true;
    };
  }, [conn, holder]);
  return conn ? dir : null;
}
