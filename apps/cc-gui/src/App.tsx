import { X } from "lucide-react";
import { useEffect, useState } from "react";
import { Composer, draftKey } from "@/features/composer";
import { Header } from "@/features/header";
import { Requests } from "@/features/requests";
import { Sidebar } from "@/features/sidebar";
import { SubagentPanel } from "@/features/subagent-panel";
import { Transcript } from "@/features/transcript";
import { api } from "@/ipc";
import { connOf, useStore, waitingKeys } from "@/state/store";

export default function App() {
  const init = useStore((s) => s.init);
  useEffect(() => init(), [init]);
  useBadge();
  return (
    <div className="flex h-screen overflow-hidden bg-ground text-text">
      <Sidebar />
      <main className="flex min-w-0 flex-1 flex-col">
        <Banners />
        <Main />
      </main>
      <SubagentPanel />
    </div>
  );
}

/** Dock badge: the number of Sessions waiting for an answer. */
function useBadge() {
  const n = useStore((s) => waitingKeys(s).length);
  useEffect(() => {
    void api.setBadge(n).catch(() => {});
  }, [n]);
}

function Banners() {
  const info = useStore((s) => s.info);
  const error = useStore((s) => s.error);
  const setError = useStore((s) => s.setError);
  const fails = info?.doctor.filter((d) => d.level === "fail") ?? [];
  const elsewhere = useElsewhere();
  const select = useStore((s) => s.select);
  return (
    <>
      {fails.map((f) => (
        <div key={f.text} className="shrink-0 bg-error-bg px-4 py-1.5 text-[12px] text-error" data-testid="doctor-fail">
          {f.text.split("\t").slice(1).join("：")}
        </div>
      ))}
      {error && (
        <div className="flex shrink-0 items-center gap-2 bg-error-bg px-4 py-1.5 text-[12px] text-error" data-testid="error">
          <span className="min-w-0 flex-1 truncate">{error}</span>
          <button type="button" aria-label="关闭" onClick={() => setError(null)}>
            <X size={12} aria-hidden />
          </button>
        </div>
      )}
      {elsewhere && (
        <button
          type="button"
          onClick={() => void select(elsewhere.key)}
          className="shrink-0 bg-selection px-4 py-1.5 text-left text-[12px] text-accent"
          data-testid="elsewhere"
        >
          {elsewhere.title} 在等你作答，点此切过去
        </button>
      )}
    </>
  );
}

/** On coming back to the window: the Session on screen has nothing to answer but another one does. */
function useElsewhere(): { key: string; title: string } | null {
  const [shown, setShown] = useState<{ key: string; title: string } | null>(null);
  useEffect(() => {
    const check = () => {
      const s = useStore.getState();
      const cur = connOf(s, s.selected);
      const others = waitingKeys(s).filter((k) => k !== s.selected);
      if ((cur && cur.requests.length > 0) || others.length === 0) return setShown(null);
      const e = s.ls.find((x) => x.holder_id === others[0]);
      setShown({ key: others[0], title: e?.title || e?.cwd || others[0] });
    };
    window.addEventListener("focus", check);
    const un = useStore.subscribe((s, p) => {
      if (s.selected !== p.selected) setShown(null);
    });
    return () => {
      window.removeEventListener("focus", check);
      un();
    };
  }, []);
  return shown;
}

function Main() {
  const c = useStore((s) => connOf(s, s.selected));
  const reconnect = useStore((s) => s.reconnect);
  const dismiss = useStore((s) => s.dismissNotice);
  if (!c)
    return (
      <div className="flex flex-1 items-center justify-center text-[13px] text-secondary" data-tauri-drag-region>
        从左边选一个 Session，或新建一个
      </div>
    );
  return (
    <>
      <Header c={c} />
      {c.procExit && (
        <div className="flex shrink-0 items-center gap-3 bg-error-bg px-4 py-1.5 text-[12px] text-error" data-testid="proc-exit">
          <span className="min-w-0 flex-1 truncate" title={c.procExit.stderr}>
            ccrpc 已退出（退出码 {c.procExit.code ?? "信号"}）{c.procExit.stderr ? `：${lastLine(c.procExit.stderr)}` : ""}
          </span>
          {c.holderId && (
            <button type="button" className="rounded border border-error px-2 py-0.5" onClick={() => void reconnect(c.conn)} data-testid="reconnect">
              重连
            </button>
          )}
        </div>
      )}
      {c.sessionExited && !c.procExit && (
        <div className="shrink-0 bg-code px-4 py-1.5 text-[12px] text-secondary" data-testid="session-exit">
          Session 已结束
        </div>
      )}
      {c.notices.map((n) => (
        <div key={n.id} className="flex shrink-0 items-center gap-2 bg-code px-4 py-1 text-[12px] text-secondary" data-testid="notice">
          <span className="min-w-0 flex-1 truncate">{n.text}</span>
          <button type="button" aria-label="关闭" onClick={() => dismiss(c.conn, n.id)}>
            <X size={12} aria-hidden />
          </button>
        </div>
      ))}
      {/* A flex column: the transcript (flex-1, scrolling) gets exactly the height left over. */}
      <div className="flex min-h-0 flex-1 flex-col">
        <Transcript conn={c.conn} />
      </div>
      <Requests c={c} />
      <Composer key={draftKey(c)} c={c} />
    </>
  );
}

const lastLine = (s: string) => s.trim().split("\n").pop() ?? "";
