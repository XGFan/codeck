import { ArrowUp, ImageIcon, X } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { api } from "@/ipc";
import type { SlashCommand, SlashSource } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { isComposingKey } from "@/lib/hotkeys";
import { queryKeys, useSlashCommands } from "@/queries";
import type { ConnState } from "./conn";
import { commandOf, matchCommands, slashCommand, typedCommand } from "./slash";
import { draftKey, EMPTY_DRAFT, useDrive, type Draft } from "./store";
import { Tasks } from "./Tasks";

const SOURCE_LABEL: Record<SlashSource, string> = { builtin: "内置", user: "用户", project: "项目", plugin: "插件" };

const extOf = (type: string) => ({ "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp" })[type];

function toBase64(f: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).replace(/^data:[^,]*,/, ""));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(f);
  });
}

/** Moves focus to the first request card's first answer (Enter while waiting). */
function focusFirstCard() {
  const answers = ["allow", "question-option", "choice-option"].map((t) => `[data-testid=drive-requests] [data-testid=${t}]:not(:disabled)`);
  document.querySelector<HTMLElement>(answers.join(", "))?.focus();
}

/**
 * The input box: idle → `prompt` (a built-in slash command → `slash`); busy → `steer`, as Enter in
 * the terminal (Claude Code's own queue, shown in the TUI, delivered when the current tool call
 * ends); waiting → not sent, focus goes to the card. Esc is handled by the drive panel. What is
 * typed lives in the drive store, so switching Sessions keeps it. Typing `/name` lists matching
 * commands: ↑↓ pick, Tab (or Enter on a different one) completes, Esc hides the list.
 */
export function Composer({ c }: { c: ConnState }) {
  const command = useDrive((s) => s.command);
  const key = draftKey(c);
  const { text, images } = useDrive((s) => s.drafts[key]) ?? EMPTY_DRAFT;
  const setDraft = useDrive((s) => s.setDraft);
  const setText = (f: string | ((t: string) => string)) => setDraft(key, (d) => ({ ...d, text: typeof f === "function" ? f(d.text) : f }));
  const setImages = (f: (x: Draft["images"]) => Draft["images"]) => setDraft(key, (d) => ({ ...d, images: f(d.images) }));
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const live = !c.procExit && !c.sessionExited && c.ready;
  const { state } = c;
  const typed = typedCommand(text);
  const { data: commands } = useSlashCommands(c.cwd, live && text.startsWith("/"));
  const qc = useQueryClient();
  // Sent before the list arrived (pasted and Enter at once): wait for it; without it, a `prompt`.
  const commandList = async () =>
    commands ??
    (await qc.fetchQuery({ queryKey: queryKeys.slashCommands(c.cwd), queryFn: () => api.listSlashCommands(c.cwd), staleTime: 30_000 }).catch(() => undefined));
  const [pick, setPick] = useState(0);
  /** The pick moved by a key (not the pointer): scroll it into view. */
  const keyMoved = useRef(false);
  const [hiddenFor, setHiddenFor] = useState<string | null>(null);
  const matches = typed != null && live && commands && hiddenFor !== text ? matchCommands(commands, typed) : [];
  const picked = matches[Math.min(pick, matches.length - 1)];
  const complete = (cmd: SlashCommand) => {
    setText(`/${cmd.name} `);
    setPick(0);
  };

  const submit = async () => {
    const body = text.trim();
    if (live && state === "waiting") return focusFirstCard();
    if (!body || sending || !live) return;
    let msg: Record<string, unknown>;
    const withImages = images.length ? { images: images.map((i) => i.path) } : {};
    let list: SlashCommand[] | undefined;
    if (body.startsWith("/")) {
      setSending(true);
      list = await commandList();
      setSending(false);
    }
    if (state === "idle") {
      const command = slashCommand(body, images.length > 0, list);
      msg = command != null ? { type: "slash", command } : { type: "prompt", text: body, ...withImages };
    } else if (state === "busy") {
      if (images.length) return setError("忙时不能发图片，等空闲时再发");
      // A panel opened from the queue would hold the turn until someone closes it in the terminal.
      const panel = commandOf(body, list);
      if (panel?.source === "builtin" && panel.interactive) return setError(`/${panel.name} 会在终端里打开界面，等空闲时再发`);
      msg = { type: "steer", text: body };
    } else return setError("Session 还没准备好");
    setSending(true);
    setError(null);
    const r = await command(c.conn, msg);
    setSending(false);
    if (!r.success) return setError(`发送失败：${r.error?.message ?? r.error?.code ?? ""}`);
    // Keep what was typed while it was being sent.
    setText((t) => (t.trim() === body ? "" : t));
    // Only the images that went with it: one pasted meanwhile stays.
    const gone = new Set(images.map((i) => i.path));
    setImages((x) => x.filter((i) => !gone.has(i.path)));
  };

  const onPaste = async (e: React.ClipboardEvent) => {
    const files = [...e.clipboardData.items].filter((i) => i.kind === "file" && extOf(i.type)).map((i) => i.getAsFile()!);
    if (files.length === 0) return;
    e.preventDefault();
    for (const f of files) {
      try {
        const b64 = await toBase64(f);
        const path = await api.driveSaveImage(b64, extOf(f.type)!);
        // A data: URL: the app's CSP allows data: images, not blob:.
        setImages((x) => [...x, { path, url: `data:${f.type};base64,${b64}` }]);
      } catch (err) {
        setError(`图片保存失败：${err && typeof err === "object" && "message" in err ? String(err.message) : String(err)}`);
      }
    }
  };

  const hint = matches.length
    ? "↑↓ 选择 · Tab 补全 · Esc 收起"
    : !live
    ? c.procExit
      ? "ccrpc 已退出"
      : c.sessionExited
        ? "Session 已结束"
        : "连接中…"
    : state === "busy"
      ? "Enter 插话，当前工具调用结束后送达 · Esc 中止"
      : state === "waiting"
        ? "先回答上面的请求 · Esc 中止"
        : state === "idle"
          ? "Enter 发送 · Shift+Enter 换行 · 可粘贴截图"
          : "启动中…";

  return (
    <div className="shrink-0" data-testid="composer">
      <div className={cn("relative rounded-xl border border-border bg-ground px-3 pt-2 pb-1.5", live ? "focus-within:border-accent" : "opacity-60")}>
        {matches.length > 0 && (
          <ul
            role="listbox"
            aria-label="斜杠命令"
            data-testid="slash-menu"
            className="absolute right-0 bottom-full left-0 mb-1 max-h-72 overflow-y-auto rounded-lg border border-border bg-ground py-1 shadow-lg"
          >
            {matches.map((m) => (
              <li
                key={m.name}
                role="option"
                aria-selected={m === picked}
                data-testid="slash-option"
                // Arrow keys move past the visible rows: keep the picked one in view.
                ref={
                  m === picked
                    ? (el) => {
                        if (el && keyMoved.current) el.scrollIntoView({ block: "nearest" });
                        keyMoved.current = false;
                      }
                    : undefined
                }
                onMouseDown={(e) => {
                  e.preventDefault();
                  complete(m);
                }}
                onMouseEnter={() => setPick(matches.indexOf(m))}
                className={cn("flex cursor-default items-baseline gap-2 px-3 py-1 text-[12px]", m === picked && "bg-selection")}
              >
                <span className="shrink-0 font-mono text-accent">/{m.name}</span>
                <span className="min-w-0 flex-1 truncate text-secondary">{m.description}</span>
                {m.interactive && (
                  <span className="shrink-0 text-[11px] text-secondary" title="会在终端里打开界面；可在请求卡片上按「关闭」退出">
                    打开界面
                  </span>
                )}
                <span className="shrink-0 text-[11px] text-secondary">{SOURCE_LABEL[m.source]}</span>
              </li>
            ))}
          </ul>
        )}
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
          value={text}
          rows={Math.min(10, Math.max(2, text.split("\n").length))}
          disabled={!live}
          onChange={(e) => {
            setText(e.target.value);
            setPick(0);
          }}
          onPaste={(e) => void onPaste(e)}
          onKeyDown={(e) => {
            if (isComposingKey(e.nativeEvent)) return;
            if (picked) {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const n = matches.length;
                keyMoved.current = true;
                setPick((matches.indexOf(picked) + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
                return;
              }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !e.metaKey && !e.ctrlKey && picked.name !== typed)) {
                e.preventDefault();
                complete(picked);
                return;
              }
              if (e.key === "Escape") {
                // Only the list: not an abort, not closing a panel.
                e.preventDefault();
                e.stopPropagation();
                setHiddenFor(text);
                return;
              }
            }
            if (e.key !== "Enter" || e.shiftKey) return;
            e.preventDefault();
            void submit();
          }}
          placeholder={state === "busy" ? "补充或排队下一条…" : "给 Claude 发消息"}
          aria-label="给 Claude 发消息"
          className="block w-full resize-none bg-transparent text-[14px] leading-[1.5] outline-none placeholder:text-secondary"
          data-testid="drive-input"
        />
        <div className="flex items-center gap-2 pt-1">
          <span className="min-w-0 flex-1 truncate text-[11px] text-secondary" data-testid="drive-hint">
            {error ? <span className="text-error">{error}</span> : hint}
          </span>
          {images.length > 0 && (
            <span className="flex items-center gap-1 text-[11px] text-secondary">
              <ImageIcon size={11} aria-hidden /> {images.length}
            </span>
          )}
          <Tasks c={c} />
          <button
            type="button"
            aria-label="发送"
            disabled={!live || !text.trim() || sending || state === "waiting" || state === "starting"}
            onClick={() => void submit()}
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
