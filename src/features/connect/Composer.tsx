import { ArrowUp, Clock, ImageIcon, MessageSquareText, Square, X } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { api } from "@/ipc";
import type { SlashCommand, SlashSource } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { isComposingKey } from "@/lib/hotkeys";
import { queryKeys, useSlashCommands } from "@/queries";
import type { ConnState } from "./conn";
import type { FeedbackDraft } from "./protocol";
import { commandOf, matchCommands, slashCommand, typedCommand } from "./slash";
import { ConnectFooter } from "./Header";
import { EMPTY_DRAFT, type Draft } from "./drafts";
import { draftKey, useConnect } from "./store";
import { ErrorNotice, NoticeCard } from "./Notice";
import { Tasks } from "./Tasks";
import { errorText } from "@/lib/errors";
import { Markdown } from "@/lib/markdown";

const SOURCE_LABEL: Record<SlashSource, string> = { builtin: "内置", user: "用户", project: "项目", plugin: "插件" };

const extOf = (type: string) =>
  ({ "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp" })[type];

function toBase64(f: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).replace(/^data:[^,]*,/, ""));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(f);
  });
}

/** Moves focus to the first request card's first answer (Enter while waiting): a card here, else a question on its transcript card. */
function focusFirstCard() {
  const el =
    document.querySelector<HTMLElement>("[data-connect-requests] [data-card-answer]:not(:disabled)") ??
    document.querySelector<HTMLElement>("[data-answering] [data-card-answer]:not(:disabled)");
  el?.focus();
}

/**
 * The model drafted product feedback (Claude Code's card above its input box): the title, the draft unfolded on
 * request, and 「不发送」 (the card's `0`). Sending stays in the terminal (`2`, or `/feedback`).
 */
function FeedbackDraftNotice({
  c,
  draft,
  onError,
}: {
  c: ConnState;
  draft: FeedbackDraft;
  onError: (e: string) => void;
}) {
  const command = useConnect((s) => s.command);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const details = draft.input?.details;
  const dismiss = async () => {
    setBusy(true);
    const r = await command(c.conn, { type: "dismiss_feedback_draft" });
    setBusy(false);
    if (!r.success) onError(`没能关掉反馈草稿：${r.error?.message ?? r.error?.code ?? ""}`);
  };
  return (
    <NoticeCard
      testId="feedback-draft"
      icon={<MessageSquareText size={18} strokeWidth={1.7} />}
      title={`Claude 起草了一条产品反馈：${draft.title}`}
      desc="草稿只存在本机，在终端里按 2 才会发送"
      action={{
        label: "不发送",
        onClick: () => void dismiss(),
        // Claude Code hides the card during a turn: only then can its key reach it.
        disabled: busy || c.state !== "idle",
        title: "关掉这张卡片（终端里的 0），草稿留在 /feedback 里，30 天后过期",
        testId: "feedback-draft-dismiss",
      }}
    >
      {details && (
        <div className="mt-1 pl-[30px] text-[12.5px]">
          <button
            type="button"
            className="text-accent hover:underline"
            onClick={() => setOpen((o) => !o)}
            data-testid="feedback-draft-toggle"
          >
            {open ? "收起" : "查看全文"}
          </button>
          {open && (
            <div data-testid="feedback-draft-details" className="mt-1 max-h-56 overflow-auto text-secondary">
              <Markdown text={details} className="text-[12.5px] leading-relaxed" />
            </div>
          )}
        </div>
      )}
    </NoticeCard>
  );
}

/**
 * The input box: idle → `prompt` (a built-in slash command → `slash`); busy → `steer`, as Enter in
 * the terminal (Claude Code's own queue, shown in the TUI and above the box, delivered when the
 * current tool call ends); ⌘Enter while busy also sends the queue at once (`send_now`, ctrl+enter in
 * the terminal: the turn is interrupted); waiting → not sent, focus goes to the card. Esc is handled by the input area. What is
 * typed lives in the connect store, so switching Sessions keeps it. Typing `/name` lists matching
 * commands: ↑↓ pick, Tab (or Enter on a different one) completes, Esc hides the list.
 */
export function Composer({ c }: { c: ConnState }) {
  const command = useConnect((s) => s.command);
  const key = draftKey(c);
  const { text, images } = useConnect((s) => s.drafts[key]) ?? EMPTY_DRAFT;
  const setDraft = useConnect((s) => s.setDraft);
  const setText = (f: string | ((t: string) => string)) =>
    setDraft(key, (d) => ({ ...d, text: typeof f === "function" ? f(d.text) : f }));
  const setImages = (f: (x: Draft["images"]) => Draft["images"]) =>
    setDraft(key, (d) => ({ ...d, images: f(d.images) }));
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
    (await qc
      .fetchQuery({
        queryKey: queryKeys.slashCommands(c.cwd),
        queryFn: () => api.listSlashCommands(c.cwd),
        staleTime: 30_000,
      })
      .catch(() => undefined));
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

  const abort = async () => {
    const r = await command(c.conn, { type: "abort" });
    if (!r.success) setError(`中止失败：${r.error?.message ?? r.error?.code ?? ""}`);
  };

  const sendNow = async () => {
    setSending(true);
    setError(null);
    const r = await command(c.conn, { type: "send_now" });
    setSending(false);
    if (!r.success) setError(`立即发送失败：${r.error?.message ?? r.error?.code ?? ""}`);
  };

  const submit = async (now = false) => {
    const body = text.trim();
    if (live && state === "waiting") return focusFirstCard();
    if (live && now && state === "busy" && !body && c.queue.length > 0 && !sending) return sendNow();
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
      // A panel opened from the queue would hold the turn until someone closes it in the terminal.
      const panel = commandOf(body, list);
      if (panel?.source === "builtin" && panel.interactive)
        return setError(`/${panel.name} 会在终端里打开界面，等空闲时再发`);
      msg = { type: "steer", text: body, ...withImages };
    } else return setError("Session 还没准备好");
    setSending(true);
    setError(null);
    const r = await command(c.conn, msg);
    setSending(false);
    if (!r.success) return setError(`发送失败：${r.error?.message ?? r.error?.code ?? ""}`);
    // ⌘Enter: what was just queued goes out now, with anything queued before it.
    if (now && msg.type === "steer" && r.data?.delivered_as === "queued") void sendNow();
    // Keep what was typed while it was being sent.
    setText((t) => (t.trim() === body ? "" : t));
    // Only the images that went with it: one pasted meanwhile stays.
    const gone = new Set(images.map((i) => i.path));
    setImages((x) => x.filter((i) => !gone.has(i.path)));
  };

  const onPaste = async (e: React.ClipboardEvent) => {
    const files = [...e.clipboardData.items]
      .filter((i) => i.kind === "file" && extOf(i.type))
      .map((i) => i.getAsFile()!);
    if (files.length === 0) return;
    e.preventDefault();
    for (const f of files) {
      try {
        const b64 = await toBase64(f);
        const path = await api.ccrpcSaveImage(b64, extOf(f.type)!);
        // A data: URL: the app's CSP allows data: images, not blob:.
        setImages((x) => [...x, { path, url: `data:${f.type};base64,${b64}` }]);
      } catch (err) {
        setError(`图片保存失败：${errorText(err)}`);
      }
    }
  };

  // What Enter does now, said in the empty box (no hint line of its own).
  const placeholder = !live
    ? "连接中…"
    : state === "busy"
      ? "插话，当前工具调用结束后送达（⌘Enter 中断本轮、立即发送，Esc 中止）"
      : state === "waiting"
        ? "先回答上面的请求（Esc 中止）"
        : state === "idle"
          ? "给 Claude 发消息（Shift+Enter 换行，可粘贴截图）"
          : "启动中…";

  return (
    <div className="shrink-0" data-testid="composer">
      <div className="mb-2 flex flex-col gap-2 empty:hidden">
        {c.queue.length > 0 && (
          <NoticeCard
            testId="queue"
            icon={<Clock size={18} strokeWidth={1.7} />}
            title={`排队 ${c.queue.length} 条`}
            desc={c.queue.map((q, i) => (
              <span key={`${i}:${q}`} data-testid="queue-item" title={q}>
                {i > 0 && "；"}
                {q}
              </span>
            ))}
            action={{
              label: "立即发送",
              onClick: () => void sendNow(),
              disabled: !live || state !== "busy" || sending,
              title: "中断本轮，立即发出排队的消息（终端里的 ctrl+enter）",
              testId: "send-now",
            }}
          />
        )}
        {c.feedbackDraft && <FeedbackDraftNotice c={c} draft={c.feedbackDraft} onError={setError} />}
        {error && <ErrorNotice text={error} onClose={() => setError(null)} />}
      </div>
      <div
        className={cn(
          "relative rounded-[18px] border border-border bg-ground pt-3 pr-2.5 pb-2 pl-4 shadow-[0_4px_16px_rgba(0,0,0,0.06)]",
          live ? "focus-within:border-text/25" : "opacity-60",
        )}
      >
        {matches.length > 0 && (
          <div
            data-testid="slash-menu"
            className="absolute right-0 bottom-full left-0 mb-1 flex flex-col overflow-hidden rounded-lg border border-border bg-ground shadow-lg"
          >
            <ul role="listbox" aria-label="斜杠命令" className="max-h-72 overflow-y-auto py-1">
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
                  className={cn(
                    "flex cursor-default items-baseline gap-2 px-3 py-1 text-[12px]",
                    m === picked && "bg-selection",
                  )}
                >
                  <span className="shrink-0 font-mono text-accent">/{m.name}</span>
                  <span className="min-w-0 flex-1 truncate text-secondary">{m.description}</span>
                  {m.interactive && (
                    <span
                      className="shrink-0 text-[11px] text-secondary"
                      title="会在终端里打开界面；可在请求卡片上按「关闭」退出"
                    >
                      打开界面
                    </span>
                  )}
                  <span className="shrink-0 text-[11px] text-secondary">{SOURCE_LABEL[m.source]}</span>
                </li>
              ))}
            </ul>
            <div className="border-t border-border px-3 py-1 text-[11px] text-secondary">
              ↑↓ 选择 · Tab 补全 · Esc 收起
            </div>
          </div>
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
              if (
                e.key === "Tab" ||
                (e.key === "Enter" && !e.shiftKey && !e.metaKey && !e.ctrlKey && picked.name !== typed)
              ) {
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
            void submit(e.metaKey || e.ctrlKey);
          }}
          placeholder={placeholder}
          aria-label="给 Claude 发消息"
          className="block w-full resize-none bg-transparent pr-1.5 text-[14px] leading-[1.5] outline-none placeholder:text-secondary"
          data-testid="connect-input"
        />
        <div className="-ml-2 flex items-center gap-1 pt-1.5">
          <ConnectFooter c={c}>
            {images.length > 0 && (
              <span className="flex h-7 items-center gap-1.5 px-2" title="粘贴的图片">
                <ImageIcon size={14} strokeWidth={1.7} aria-hidden /> {images.length}
              </span>
            )}
            <Tasks c={c} />
          </ConnectFooter>
          {/* Busy with nothing typed: stop (Esc in the terminal) instead of send. */}
          {live && !text.trim() && (state === "busy" || state === "waiting") ? (
            <button
              type="button"
              aria-label="中止"
              title="中止本轮（Esc）"
              onClick={() => void abort()}
              className="ml-1 flex size-8 shrink-0 items-center justify-center rounded-full bg-text text-ground hover:opacity-85"
              data-testid="stop"
            >
              <Square size={11} fill="currentColor" aria-hidden />
            </button>
          ) : (
            <button
              type="button"
              aria-label="发送"
              disabled={!live || !text.trim() || sending || state === "waiting" || state === "starting"}
              onClick={() => void submit()}
              className="ml-1 flex size-8 shrink-0 items-center justify-center rounded-full bg-accent text-white disabled:bg-border disabled:text-ground"
              data-testid="send"
            >
              <ArrowUp size={16} strokeWidth={2.2} aria-hidden />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
