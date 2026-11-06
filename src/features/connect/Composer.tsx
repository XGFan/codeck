import { ArrowUp, Clock, File, Folder, MessageSquareText, Square } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "@/ipc";
import type { SlashCommand, SlashSource } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { isComposingKey } from "@/lib/hotkeys";
import { queryKeys, usePathCompletions, useSlashCommands } from "@/queries";
import type { ConnState } from "./conn";
import type { FeedbackDraft } from "./protocol";
import { applyPath, commandOf, matchCommands, pathAt, slashCommand, typedCommand, typedName } from "./slash";
import { ConnectFooter } from "./Header";
import { EMPTY_DRAFT, isDraft, type Draft } from "./drafts";
import { draftKey, useConnect } from "./store";
import { ErrorNotice, NoticeCard } from "./Notice";
import { markFresh } from "./pending";
import { scrollToBottom } from "@/features/transcript/nodes/scroll";
import { RewindPickNotice } from "./RewindViews";
import { Tasks } from "./Tasks";
import { errorText } from "@/lib/errors";
import { Markdown } from "@/lib/markdown";
import { caretOnFirstRow } from "./caretRow";
import { editable, queueText, rememberSteer, takenBack } from "./queue";
import { ChipLayer, MirrorText } from "./ImageChips";
import {
  dropImages,
  hasTokens,
  moveToken,
  normalize,
  plain,
  shiftTokens,
  snap,
  splice,
  tokenAfter,
  tokenBefore,
  tokensIn,
  token,
  toParts,
  widen,
  type Tok,
} from "./imageTokens";

/** Copied tokens carry their images along (pasted back into an input box, they come back). */
const TOKENS_MIME = "application/x-codeck-images";

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
 * The input box: idle → `prompt` (a built-in slash command → `slash`; `/rewind` (`/checkpoint`) → 「选回退点」, nothing
 * sent); busy → `steer`, as Enter in the terminal (Claude Code's own queue, shown in the TUI and above the box,
 * delivered when the current tool call ends); ⌘Enter while busy also sends the queue at once (`send_now`, ctrl+enter in
 * the terminal: the turn is interrupted); ↑ on the first line while busy takes what waits in the queue back into the box
 * (`edit_queue`, ↑ in the terminal); waiting → not sent, focus goes to the card. Esc is handled by the input area. What is
 * typed lives in the connect store, so switching Sessions keeps it. Typing `/name` lists matching
 * commands: ↑↓ pick, Tab (or Enter on a different one) completes, Esc hides the list.
 */
export function Composer({ c }: { c: ConnState }) {
  const command = useConnect((s) => s.command);
  const answerRewind = useConnect((s) => s.answerRewind);
  const setPicking = useConnect((s) => s.setPicking);
  const rewoundSent = useConnect((s) => s.rewoundSent);
  const setPending = useConnect((s) => s.setPending);
  const key = draftKey(c);
  const { text, images } = useConnect((s) => s.drafts[key]) ?? EMPTY_DRAFT;
  const setDraft = useConnect((s) => s.setDraft);
  // Every change keeps the tokens and the images in step: renumbered in text order, an image without its token gone.
  const update = (f: (d: Draft) => Draft) =>
    setDraft(key, (d) => {
      const n = f(d);
      return { ...n, ...normalize(n.text, n.images) };
    });
  /** Replaces `[start, end)` with `insert` (and the images with `images`, when given); the caret goes after it. */
  const edit = (start: number, end: number, insert: string, images?: (x: Draft["images"]) => Draft["images"]) => {
    update((d) => {
      const r = splice(d.text, start, end, insert);
      caretAfter.current = r.caret;
      return { ...d, text: r.text, images: images ? images(d.images) : d.images };
    });
  };
  const setText = (f: string | ((t: string) => string)) =>
    update((d) => ({ ...d, text: typeof f === "function" ? f(d.text) : f }));
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
  // How much of the text is a known command's `/name`.
  // Not while an input method composes: the textarea's own text carries the marked-text underline.
  const [composing, setComposing] = useState(false);
  const named = live && !composing ? commandOf(text, commands) : null;
  const highlight = named ? typedName(text).length + 1 : 0;
  const mirror = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const mirrored = highlight > 0 || hasTokens(text);
  // The copy wraps and shows the lines the textarea does: as wide as its text (a scroll bar, index.css gives it
  // width, takes some once the text overflows), at its scroll also when it appears on a textarea already scrolled.
  useLayoutEffect(() => {
    const m = mirror.current;
    const t = input.current;
    if (!m || !t) return;
    m.style.right = `${t.offsetWidth - t.clientWidth}px`;
    m.scrollTop = t.scrollTop;
  }, [text, highlight, mirrored]);
  const complete = (cmd: SlashCommand) => {
    setText(`/${cmd.name} `);
    setPick(0);
  };
  // A path being typed (`/add-dir <dir>`, `@file`): what is there (relative to the Session's cwd, `../` and `~/`
  // as in a shell).
  const [caret, setCaret] = useState(0);
  const at = live ? pathAt(text, caret) : null;
  const { data: paths } = usePathCompletions(c.cwd, at?.partial ?? "", !!at?.files, at != null);
  // The list kept from the last path while this one loads: only what still fits, and not what is typed already.
  const pathMatches =
    at && paths && hiddenFor !== text
      ? paths.filter((d) => {
          const [l, p] = [d.toLowerCase(), at.partial.toLowerCase()];
          return l.startsWith(p) && l !== p;
        })
      : [];
  const pickedPath = pathMatches[Math.min(pick, pathMatches.length - 1)];
  /** ↑↓ moved the pick since the last edit: Enter takes it (otherwise Enter sends what is typed). */
  const [navigated, setNavigated] = useState(false);
  /** Where the caret goes once a completion is in the box. */
  const caretAfter = useRef<number | null>(null);
  const completePath = (d: string) => {
    if (!at) return;
    const r = applyPath(text, at, d);
    setText(r.text);
    setCaret(r.caret);
    caretAfter.current = r.caret;
    setPick(0);
    setNavigated(false);
  };
  useLayoutEffect(() => {
    const n = caretAfter.current;
    if (n == null || !input.current) return;
    caretAfter.current = null;
    input.current.setSelectionRange(n, n);
  }, [text]);
  // A fresh list: a pick made on the one kept meanwhile may point elsewhere now.
  useEffect(() => {
    setPick(0);
    setNavigated(false);
  }, [paths]);

  const abort = async () => {
    // ccrpc is in the terminal's Rewind picker: not to be interrupted.
    if (c.rewinding) return;
    // The rewind's confirm page: cancelled (an abort would only step back to the terminal's picker).
    const rw = c.requests.find((r) => r.type === "rewind");
    if (rw) return answerRewind(c.conn, rw, null);
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

  /** ↑ takes what the person queued back (as in the terminal): there is some, and nothing else is on its way. */
  const canEditQueue = live && state === "busy" && !sending && c.queue.some(editable);
  const editQueue = async () => {
    setSending(true);
    setError(null);
    const r = await command(c.conn, { type: "edit_queue" });
    setSending(false);
    const items = r.data?.items;
    if (!r.success || !Array.isArray(items))
      return setError(`取回排队的消息失败：${r.error?.message ?? r.error?.code ?? ""}`);
    const back = takenBack(c.conn, items as string[]);
    if (!back.text) return;
    // Before what is in the box, as in the terminal; the caret after what came back.
    update((d) => {
      caretAfter.current = back.text.length;
      const after = shiftTokens(d.text, back.images.length);
      return { text: d.text.trim() ? `${back.text}\n${after}` : back.text, images: [...back.images, ...d.images] };
    });
  };

  const submit = async (now = false) => {
    const body = text.trim();
    if (live && state === "waiting") return focusFirstCard();
    if (live && now && state === "busy" && !body && c.queue.length > 0 && !sending) return sendNow();
    if (!body || sending || !live) return;
    // Not for the terminal: the prompt to go back to is picked in the transcript.
    if (state === "idle" && /^\/(rewind|checkpoint)(\s|$)/.test(body)) {
      setPicking(c.conn, true);
      setText((t) => (t.trim() === body ? "" : t));
      return;
    }
    if (c.picking) setPicking(c.conn, false);
    let msg: Record<string, unknown>;
    // With images: the words and the images in the order they are in the box (`parts`), else the text alone.
    const content = images.length ? { parts: toParts(body, images) } : { text: body };
    let list: SlashCommand[] | undefined;
    if (body.startsWith("/")) {
      setSending(true);
      list = await commandList();
      setSending(false);
    }
    if (state === "idle") {
      const command = slashCommand(body, images.length > 0, list);
      msg = command != null ? { type: "slash", command } : { type: "prompt", ...content };
    } else if (state === "busy") {
      // A panel opened from the queue would hold the turn until someone closes it in the terminal.
      const panel = commandOf(body, list);
      if (panel?.source === "builtin" && panel.interactive)
        return setError(`/${panel.name} 会在终端里打开界面，等空闲时再发`);
      msg = { type: "steer", ...content };
    } else return setError("Session 还没准备好");
    setSending(true);
    setError(null);
    // Sent: to the end of the transcript (its placeholder, the queue, the reply), as a chat app does.
    scrollToBottom();
    const prompt = msg.type === "prompt";
    const sent = images;
    const gone = new Set(sent.map((i) => i.path));
    const at = Date.now();
    if (prompt) {
      // A placeholder at the transcript's end at once, and the box is free again (both back if it fails).
      setPending(c.conn, { uuid: null, at });
      // What was typed meanwhile stays: without the sent images (their tokens by image, not by number).
      update((d) => (d.text.trim() === body ? { text: "", images: [] } : dropImages(d.text, d.images, gone)));
    }
    const r = await command(c.conn, msg);
    setSending(false);
    if (!r.success) {
      if (prompt) {
        setPending(c.conn, null);
        // Back in the box, its images first (what was typed meanwhile numbered after them).
        update((d) => {
          const rest = d.images.filter((i) => !gone.has(i.path));
          const after = shiftTokens(d.text, sent.length);
          return { text: d.text.trim() ? `${body}\n${after}` : body, images: [...sent, ...rest] };
        });
      }
      return setError(`发送失败：${r.error?.message ?? r.error?.code ?? ""}`);
    }
    if (prompt) {
      // Into Claude Code's queue (the turn had not quite ended): shown in the queue card, not as a prompt yet.
      const uuid = r.data?.delivered_as !== "queued" && typeof r.data?.uuid === "string" ? r.data.uuid : null;
      if (uuid) markFresh(uuid);
      setPending(c.conn, uuid ? { uuid, at } : null);
      // A new prompt: what was rewound becomes another Branch (the marker goes once that is indexed).
      rewoundSent(c.conn);
      // It is in the transcript by now: indexed at once, not after the watcher's debounce.
      if (c.sessionId) void api.syncSession(c.sessionId).catch(() => {});
      return;
    }
    // Its images, should ↑ take it back (the queue has only `[Image #n]` for them).
    if (msg.type === "steer" && r.data?.delivered_as === "queued") rememberSteer(c.conn, { text: body, images: sent });
    // ⌘Enter: what was just queued goes out now, with anything queued before it.
    if (now && msg.type === "steer" && r.data?.delivered_as === "queued") void sendNow();
    // Keep what was typed while it was being sent; only the images that went with it go (an image pasted meanwhile
    // keeps its place).
    update((d) => (d.text.trim() === body ? { text: "", images: [] } : dropImages(d.text, d.images, gone)));
  };

  /** The selection, a part of a token counting as all of it. */
  const selection = (el: HTMLTextAreaElement) => widen(text, el.selectionStart, el.selectionEnd);

  const onPaste = async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const [start, end] = selection(e.currentTarget);
    // Tokens copied from an input box: back with their images, numbered after the ones here.
    const copied = e.clipboardData.getData(TOKENS_MIME);
    if (copied) {
      try {
        const v: unknown = JSON.parse(copied);
        if (isDraft(v)) {
          e.preventDefault();
          update((d) => {
            const r = splice(d.text, start, end, shiftTokens(v.text, d.images.length));
            caretAfter.current = r.caret;
            return { text: r.text, images: [...d.images, ...v.images] };
          });
          return;
        }
      } catch {
        // Not ours after all: pasted as text.
      }
    }
    const files = [...e.clipboardData.items]
      .filter((i) => i.kind === "file" && extOf(i.type))
      .map((i) => i.getAsFile()!);
    if (files.length === 0) return;
    e.preventDefault();
    const saved: Draft["images"] = [];
    for (const f of files) {
      try {
        const b64 = await toBase64(f);
        const path = await api.ccrpcSaveImage(b64, extOf(f.type)!);
        // A data: URL: the app's CSP allows data: images, not blob:.
        saved.push({ path, url: `data:${f.type};base64,${b64}` });
      } catch (err) {
        setError(`图片保存失败：${errorText(err)}`);
      }
    }
    if (saved.length === 0) return;
    // A token where the caret was for each, numbered after the images already there. Typed into meanwhile (a big
    // image takes a moment to save): where the caret is now, never inside another token.
    const pasted = text;
    update((d) => {
      const tokens = saved.map((_, i) => token(d.images.length + i + 1)).join("");
      const cur = input.current?.selectionStart ?? d.text.length;
      const [s0, e0] = d.text === pasted ? [start, end] : [cur, cur];
      const at = snap(d.text, Math.min(s0, d.text.length), 0);
      const r = splice(d.text, at, Math.max(at, snap(d.text, Math.min(e0, d.text.length), 0)), tokens);
      caretAfter.current = r.caret;
      return { text: r.text, images: [...d.images, ...saved] };
    });
  };

  /** Copied or cut text with tokens: as 「[图片 n]」 for other apps, with the images for an input box. */
  const onCopy = (e: React.ClipboardEvent<HTMLTextAreaElement>, cut: boolean) => {
    const [start, end] = selection(e.currentTarget);
    const piece = text.slice(start, end);
    if (!hasTokens(piece)) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", plain(piece));
    e.clipboardData.setData(TOKENS_MIME, JSON.stringify(normalize(piece, images)));
    if (cut) edit(start, end, "");
  };

  /** A token dragged to `to`: moved there, the images following the new order. */
  const moveTo = (tok: Tok, to: number) =>
    update((d) => {
      const r = moveToken(d.text, tok, to);
      caretAfter.current = r.caret;
      input.current?.focus();
      return { ...d, text: r.text };
    });

  /** Where the selection is (the token it covers exactly is drawn selected). */
  const [sel, setSel] = useState<[number, number]>([0, 0]);
  const lastCaret = useRef(0);
  const selectedToken = tokensIn(text).find((t) => t.start === sel[0] && t.end === sel[1])?.n ?? null;

  // What Enter does now, said in the empty box (no hint line of its own).
  const placeholder = !live
    ? "连接中…"
    : state === "busy"
      ? c.queue.some(editable)
        ? "按 ↑ 取回排队的消息（⌘Enter 中断本轮、立即发送，Esc 中止）"
        : "插话，当前工具调用结束后送达（⌘Enter 中断本轮、立即发送，Esc 中止）"
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
              <span key={`${i}:${q}`} data-testid="queue-item" title={queueText(q)}>
                {i > 0 && "；"}
                {queueText(q)}
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
        {c.picking && <RewindPickNotice c={c} />}
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
                          // Not on detaching (null, before the next pick attaches): that would drop the move.
                          if (!el) return;
                          if (keyMoved.current) el.scrollIntoView({ block: "nearest" });
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
        {pathMatches.length > 0 && (
          <div
            data-testid="path-menu"
            className="absolute right-0 bottom-full left-0 mb-1 flex flex-col overflow-hidden rounded-lg border border-border bg-ground shadow-lg"
          >
            <ul role="listbox" aria-label="路径" className="max-h-72 overflow-y-auto py-1">
              {pathMatches.map((d) => (
                <li
                  key={d}
                  role="option"
                  aria-selected={d === pickedPath}
                  data-testid="path-option"
                  ref={
                    d === pickedPath
                      ? (el) => {
                          // Not on detaching (null, before the next pick attaches): that would drop the move.
                          if (!el) return;
                          if (keyMoved.current) el.scrollIntoView({ block: "nearest" });
                          keyMoved.current = false;
                        }
                      : undefined
                  }
                  onMouseDown={(e) => {
                    e.preventDefault();
                    completePath(d);
                  }}
                  onMouseEnter={() => setPick(pathMatches.indexOf(d))}
                  className={cn(
                    "flex cursor-default items-center gap-2 px-3 py-1 font-mono text-[12px]",
                    d === pickedPath && "bg-selection",
                  )}
                >
                  {d.endsWith("/") ? (
                    <Folder size={13} strokeWidth={1.7} className="shrink-0 text-secondary" aria-hidden />
                  ) : (
                    <File size={13} strokeWidth={1.7} className="shrink-0 text-secondary" aria-hidden />
                  )}
                  <span className="min-w-0 truncate">{d}</span>
                </li>
              ))}
            </ul>
            <div className="border-t border-border px-3 py-1 text-[11px] text-secondary">
              ↑↓ 选择 · Tab 补全 · Enter 发送 · Esc 收起
            </div>
          </div>
        )}
        <div ref={box} className="relative">
          {/* The command named at the start in color, the images as blocks: drawn on a copy of the text under the
              (then clear) textarea. */}
          {mirrored && (
            <div
              ref={mirror}
              aria-hidden
              data-testid={highlight > 0 ? "command-highlight" : "composer-mirror"}
              className={cn(
                "pointer-events-none absolute inset-0 overflow-hidden pr-1.5 text-[14px] leading-[1.5] break-words whitespace-pre-wrap",
                composing && "text-transparent",
              )}
            >
              <MirrorText text={text} highlight={highlight} selected={selectedToken} ghost={composing} />
              {/* A last empty line still takes its height, as in the textarea. */}
              {"\u200b"}
            </div>
          )}
          <textarea
            ref={input}
            value={text}
            onSelect={(e) => {
              const el = e.currentTarget;
              // The user moved the caret: a caret position waiting for a change that did not come (a token dropped
              // where it was) is void.
              if (caretAfter.current != null && el.value === text) caretAfter.current = null;
              let [start, end] = [el.selectionStart, el.selectionEnd];
              // A caret never stays inside an image token: on to its end the way it moved.
              if (start === end) {
                const p = snap(el.value, start, Math.sign(start - lastCaret.current));
                if (p !== start) el.setSelectionRange(p, p);
                start = end = p;
                lastCaret.current = p;
              }
              setSel([start, end]);
              setCaret(start);
            }}
            onCopy={(e) => onCopy(e, false)}
            onCut={(e) => onCopy(e, true)}
            onCompositionStart={() => setComposing(true)}
            onCompositionEnd={() => setComposing(false)}
            rows={Math.min(10, Math.max(2, text.split("\n").length))}
            disabled={!live}
            onChange={(e) => {
              setText(e.target.value);
              setCaret(e.target.selectionStart);
              setPick(0);
              setNavigated(false);
            }}
            onPaste={(e) => void onPaste(e)}
            onKeyDown={(e) => {
              if (isComposingKey(e.nativeEvent)) return;
              // ⌫ / Delete take an image token whole (and its image); a selection over part of one, all of it.
              if ((e.key === "Backspace" || e.key === "Delete") && !e.metaKey && !e.altKey && !e.ctrlKey) {
                const el = e.currentTarget;
                const [s0, e0] = [el.selectionStart, el.selectionEnd];
                if (s0 === e0) {
                  const t = e.key === "Backspace" ? tokenBefore(el.value, s0) : tokenAfter(el.value, s0);
                  if (t) {
                    e.preventDefault();
                    edit(t.start, t.end, "");
                    return;
                  }
                } else {
                  const [s1, e1] = widen(el.value, s0, e0);
                  if (s1 !== s0 || e1 !== e0) {
                    e.preventDefault();
                    edit(s1, e1, "");
                    return;
                  }
                }
              }
              if (pickedPath) {
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  const n = pathMatches.length;
                  keyMoved.current = true;
                  setNavigated(true);
                  setPick((pathMatches.indexOf(pickedPath) + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
                  return;
                }
                if (e.key === "Tab" || (e.key === "Enter" && navigated && !e.shiftKey && !e.metaKey && !e.ctrlKey)) {
                  e.preventDefault();
                  completePath(pickedPath);
                  return;
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  e.stopPropagation();
                  setHiddenFor(text);
                  return;
                }
              }
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
              // ↑ on the first row while messages wait in the queue: back into the box to edit (↑ in the terminal).
              if (e.key === "ArrowUp" && !e.shiftKey && !e.metaKey && !e.altKey && !e.ctrlKey && canEditQueue) {
                if (caretOnFirstRow(e.currentTarget)) {
                  e.preventDefault();
                  void editQueue();
                  return;
                }
              }
              if (e.key !== "Enter" || e.shiftKey) return;
              e.preventDefault();
              void submit(e.metaKey || e.ctrlKey);
            }}
            placeholder={placeholder}
            aria-label="给 Claude 发消息"
            onScroll={(e) => {
              if (mirror.current) mirror.current.scrollTop = e.currentTarget.scrollTop;
            }}
            className={cn(
              "relative block w-full resize-none bg-transparent pr-1.5 text-[14px] leading-[1.5] outline-none placeholder:text-secondary",
              // While an input method composes, the textarea's own text (it draws the marked-text underline).
              mirrored && !composing && "text-transparent caret-text",
            )}
            data-testid="connect-input"
          />
          {hasTokens(text) && (
            <ChipLayer text={text} images={images} mirror={mirror} input={input} box={box} onMove={moveTo} />
          )}
        </div>
        <div className="-ml-2 flex items-center gap-1 pt-1.5">
          <ConnectFooter c={c}>
            <Tasks c={c} />
          </ConnectFooter>
          {/* Busy with nothing typed: stop (Esc in the terminal) instead of send. */}
          {live && !text.trim() && (state === "busy" || state === "waiting") ? (
            <button
              type="button"
              aria-label="中止"
              title="中止本轮（Esc）"
              disabled={!!c.rewinding}
              onClick={() => void abort()}
              className="ml-1 flex size-8 shrink-0 items-center justify-center rounded-full bg-text text-ground hover:opacity-85 disabled:opacity-40"
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
      {c.rewound?.keptDraft && (
        <p data-testid="rewind-kept-draft" className="m-0 mt-1.5 pl-4 text-[12px] text-secondary">
          输入框里已有内容，原提示词没有填入
        </p>
      )}
    </div>
  );
}
