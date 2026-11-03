import { ArrowUp, ImageIcon, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "@/ipc";
import { isComposingKey } from "@/lib/hotkeys";
import { cn } from "@/lib/cn";
import type { ConnState } from "@/state/conn";
import { useStore } from "@/state/store";

interface Img {
  path: string;
  url: string;
}

const extOf = (type: string) => ({ "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp" })[type];

function toBase64(f: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).replace(/^data:[^,]*,/, ""));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(f);
  });
}

/**
 * The input box (docs/spec.md §5): idle → `prompt`; busy → `follow_up` (⌘Enter `steer`);
 * waiting → not sent. Esc aborts while busy / waiting.
 */
export function Composer({ c }: { c: ConnState }) {
  const command = useStore((s) => s.command);
  const [text, setText] = useState("");
  const [images, setImages] = useState<Img[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null);
  const live = !c.procExit && !c.sessionExited && c.ready;
  const { state } = c;

  useEffect(() => {
    setText("");
    setImages([]);
    setError(null);
  }, [c.conn]);

  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || isComposingKey(e) || !live) return;
      // Esc in a card's text field (feedback, "other") only leaves the field.
      if ((e.target as HTMLElement | null)?.tagName === "INPUT") return;
      if (state !== "busy" && state !== "waiting") return;
      if (document.querySelector("[data-radix-popper-content-wrapper], [role=dialog]")) return;
      e.preventDefault();
      void command(c.conn, { type: "abort" }).then((r) => {
        if (!r.success) setError(`中止失败：${r.error?.message ?? ""}`);
      });
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [c.conn, command, live, state]);

  const submit = async (steer: boolean) => {
    const body = text.trim();
    if (!body || sending || !live) return;
    let msg: Record<string, unknown>;
    if (state === "idle") msg = { type: "prompt", text: body, ...(images.length ? { images: images.map((i) => i.path) } : {}) };
    else if (state === "busy" && steer) {
      if (images.length) return setError("插话不能带图片：去掉图片，或按 Enter 排到本轮之后");
      msg = { type: "steer", text: body };
    } else if (state === "busy") msg = { type: "follow_up", text: body, ...(images.length ? { images: images.map((i) => i.path) } : {}) };
    else return setError(state === "waiting" ? "先回答上面的请求" : "现在还不能发送");
    setSending(true);
    setError(null);
    const r = await command(c.conn, msg);
    setSending(false);
    if (!r.success) return setError(`发送失败：${r.error?.message ?? r.error?.code ?? ""}`);
    if (msg.type === "follow_up") void refreshQueue();
    // Keep what was typed while it was being sent.
    setText((t) => (t.trim() === body ? "" : t));
    images.forEach((i) => URL.revokeObjectURL(i.url));
    setImages([]);
  };

  const setQueue = (f: string[]) =>
    useStore.setState((s) => (s.conns[c.conn] ? { conns: { ...s.conns, [c.conn]: { ...s.conns[c.conn], followUps: f } } } : {}));

  const refreshQueue = async () => {
    const r = await command(c.conn, { type: "get_state" });
    if (r.success && Array.isArray(r.data?.follow_ups)) setQueue(r.data.follow_ups as string[]);
  };

  const cancel = async (index: number) => {
    const r = await command(c.conn, { type: "cancel_follow_up", index });
    if (r.success && Array.isArray(r.data?.follow_ups)) setQueue(r.data.follow_ups as string[]);
  };

  const onPaste = async (e: React.ClipboardEvent) => {
    const files = [...e.clipboardData.items].filter((i) => i.kind === "file" && extOf(i.type)).map((i) => i.getAsFile()!);
    if (files.length === 0) return;
    e.preventDefault();
    for (const f of files) {
      try {
        const path = await api.saveImage(await toBase64(f), extOf(f.type)!);
        setImages((x) => [...x, { path, url: URL.createObjectURL(f) }]);
      } catch (err) {
        setError(`存不了图片：${String(err)}`);
      }
    }
  };

  const hint = !live
    ? c.procExit
      ? "ccrpc 已退出"
      : c.sessionExited
        ? "Session 已结束"
        : "连接中…"
    : state === "busy"
      ? "Enter 排到本轮之后 · ⌘Enter 立即插话 · Esc 中止"
      : state === "waiting"
        ? "先回答上面的请求 · Esc 中止"
        : state === "idle"
          ? "Enter 发送 · Shift+Enter 换行 · 可粘贴截图"
          : "启动中…";

  return (
    <div className="shrink-0 px-4 pb-3" data-testid="composer">
      {c.progress && state === "busy" && (
        <div className="mb-1.5 truncate text-[12px] text-claude" data-testid="progress">
          {c.progress.text}
        </div>
      )}
      {c.followUps.length > 0 && (
        <ul className="mb-1.5 space-y-1" data-testid="follow-ups">
          {c.followUps.map((f, i) => (
            <li key={`${i}:${f}`} className="flex items-center gap-2 rounded border border-dashed border-border px-2 py-1 text-[12px]">
              <span className="shrink-0 text-secondary">排队</span>
              <span className="min-w-0 flex-1 truncate">{f}</span>
              <button type="button" aria-label="撤销" onClick={() => void cancel(i)} className="text-secondary hover:text-text" data-testid="cancel-follow-up">
                <X size={12} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className={cn("rounded-xl border bg-ground px-3 pt-2 pb-1.5", live ? "border-border focus-within:border-accent" : "border-border opacity-60")}>
        {images.length > 0 && (
          <div className="mb-1.5 flex flex-wrap gap-1.5" data-testid="images">
            {images.map((img, i) => (
              <span key={img.path} className="relative">
                <img src={img.url} alt="" className="size-14 rounded border border-border object-cover" />
                <button
                  type="button"
                  aria-label="去掉图片"
                  onClick={() => setImages((x) => x.filter((_, j) => j !== i))}
                  className="absolute -top-1.5 -right-1.5 flex size-4 items-center justify-center rounded-full bg-text text-ground"
                >
                  <X size={10} aria-hidden />
                </button>
              </span>
            ))}
          </div>
        )}
        <textarea
          ref={box}
          value={text}
          rows={Math.min(10, Math.max(2, text.split("\n").length))}
          disabled={!live}
          onChange={(e) => setText(e.target.value)}
          onPaste={(e) => void onPaste(e)}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || e.shiftKey || isComposingKey(e.nativeEvent)) return;
            e.preventDefault();
            void submit(e.metaKey || e.ctrlKey);
          }}
          placeholder={state === "busy" ? "补充或排队下一条…" : "给 Claude 发消息"}
          className="block w-full resize-none bg-transparent text-[14px] leading-[1.5] outline-none placeholder:text-secondary"
          data-testid="input"
        />
        <div className="flex items-center gap-2 pt-1">
          <span className="min-w-0 flex-1 truncate text-[11px] text-secondary" data-testid="hint">
            {error ? <span className="text-error">{error}</span> : hint}
          </span>
          {images.length > 0 && (
            <span className="flex items-center gap-1 text-[11px] text-secondary">
              <ImageIcon size={11} aria-hidden /> {images.length}
            </span>
          )}
          <button
            type="button"
            aria-label="发送"
            disabled={!live || !text.trim() || sending || state === "waiting" || state === "starting"}
            onClick={() => void submit(false)}
            className="flex size-6 items-center justify-center rounded-full bg-accent text-white disabled:opacity-30"
            data-testid="send"
          >
            <ArrowUp size={14} aria-hidden />
          </button>
        </div>
      </div>
    </div>
  );
}
