import { ChevronDown, ShipWheel, Unlink } from "lucide-react";
import { useEffect, useState } from "react";
import { CopyButton } from "@/components/ui/copy-button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { api } from "@/ipc";
import { cn } from "@/lib/cn";
import { formatTokens } from "@/lib/format";
import type { ConnState } from "./conn";
import { connFor, errorKey, holderConnected, lsEntryFor, useDrive } from "./store";

const MODES: { id: string; label: string }[] = [
  { id: "manual", label: "手动审批" },
  { id: "accept_edits", label: "自动接受编辑" },
  { id: "plan", label: "计划模式" },
  { id: "auto", label: "自动模式" },
];

const STATE_LABEL: Record<string, string> = {
  starting: "启动中",
  idle: "空闲",
  busy: "工作中",
  waiting: "等待作答",
  exited: "已结束",
};

const pill = "flex h-7 items-center gap-1.5 rounded-md border border-border bg-ground px-2.5 text-[12px] hover:bg-selection disabled:opacity-50 disabled:hover:bg-ground";

const smallPill = "flex h-6 items-center gap-1 rounded-md border border-border bg-ground px-2 text-[12px] hover:bg-selection disabled:opacity-50 disabled:hover:bg-ground";

/**
 * 「驾驶」 in the header's title row, for a Session in `ccrpc ls` that is not connected (disabled until its ccrpc
 * plugin has reported). Nothing once connected: the controls move to their own row ([`DriveBar`]).
 */
export function DriveConnect({ sessionId }: { sessionId: string }) {
  const entry = useDrive((s) => lsEntryFor(s, sessionId));
  const c = useDrive((s) => connFor(s, sessionId));
  const taken = useDrive((s) => (entry ? holderConnected(s, entry.holder_id) : false));
  const connect = useDrive((s) => s.connect);
  const error = useDrive((s) => s.errors[sessionId]);
  const [connecting, setConnecting] = useState(false);

  if (c && !c.procExit) return null;
  if (!entry || (taken && !c)) return error ? <DriveError errKey={sessionId} text={error} /> : null;
  return (
    <>
      {error && <DriveError errKey={sessionId} text={error} />}
      <button
        type="button"
        data-testid="drive-connect"
        disabled={!entry.plugin || connecting}
        title={entry.plugin ? "经 ccrpc 连接，在这里发提示词、作答审批" : "ccrpc 插件未加载，重启 claude 后可驾驶"}
        onClick={async () => {
          setConnecting(true);
          await connect(entry.holder_id, sessionId);
          setConnecting(false);
        }}
        className={pill}
      >
        <ShipWheel size={13} strokeWidth={1.6} aria-hidden />
        驾驶
      </button>
    </>
  );
}

function DriveError({ errKey, text }: { errKey: string; text: string }) {
  const setError = useDrive((s) => s.setError);
  return (
    <button type="button" data-testid="drive-error" title="关闭" onClick={() => setError(errKey, null)} className="max-w-72 truncate text-[12px] text-error">
      {text}
    </button>
  );
}

/** The header's row for a connected Session: state, model, context tokens, permission mode, 「断开」, debug info. */
export function DriveBar({ sessionId }: { sessionId: string }) {
  const c = useDrive((s) => connFor(s, sessionId));
  if (!c || c.procExit) return null;
  return <Connected c={c} />;
}

function Connected({ c }: { c: ConnState }) {
  const command = useDrive((s) => s.command);
  const disconnect = useDrive((s) => s.disconnect);
  const setError = useDrive((s) => s.setError);
  const error = useDrive((s) => s.errors[errorKey(c)]);
  const entry = useDrive((s) => lsEntryFor(s, c.sessionId));
  const live = !c.sessionExited;
  const mode = MODES.find((m) => m.id === c.mode);
  const logDir = useLogDir(c);
  const cwd = c.cwd ?? entry?.cwd ?? null;
  const debugInfo = [`holder_id: ${c.holderId ?? "-"}`, `session_id: ${c.sessionId ?? "-"}`, `cwd: ${cwd ?? "-"}`, `logs: ${logDir ?? "-"}`].join("\n");
  const setMode = async (id: string) => {
    const r = await command(c.conn, { type: "set_mode", mode: id });
    if (!r.success) setError(errorKey(c), `切换权限模式失败：${r.error?.message ?? ""}`);
  };
  return (
    <div data-testid="drive-controls" className="group/copy flex flex-wrap items-center gap-x-3.5 gap-y-1 text-[12px] text-secondary">
      <span data-testid="drive-state" data-state={c.state} className={cn("flex items-center gap-1.5 font-semibold text-text/80", c.state === "waiting" && "text-live-waiting")}>
        <ShipWheel size={13} strokeWidth={1.6} aria-hidden />
        驾驶中 · {STATE_LABEL[c.state] ?? c.state}
      </span>
      {c.model && <span>{c.model}</span>}
      {c.contextTokens != null && <span title="上下文 token">上下文 {formatTokens(c.contextTokens)}</span>}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" data-testid="drive-mode" disabled={!live || !(c.state === "idle" || c.state === "busy")} className={smallPill}>
            {mode?.label ?? c.mode ?? "权限模式"}
            <ChevronDown size={12} aria-hidden />
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
      <button type="button" data-testid="drive-disconnect" onClick={() => void disconnect(c.conn)} className={smallPill}>
        <Unlink size={12} strokeWidth={1.6} aria-hidden />
        断开
      </button>
      <CopyButton text={debugInfo} label="复制排查信息" />
      {error && <DriveError errKey={errorKey(c)} text={error} />}
    </div>
  );
}

/** The connection's log directory; asked again once a new Session's directory is renamed after `ready`. */
function useLogDir(c: ConnState): string | null {
  const [dir, setDir] = useState<string | null>(null);
  useEffect(() => {
    let gone = false;
    void api.driveLogDir(c.conn).then(
      (d) => !gone && setDir(d),
      () => {},
    );
    return () => {
      gone = true;
    };
  }, [c.conn, c.holderId]);
  return dir;
}
