import { ChevronDown, ChevronRight, CircleX, ImageIcon, Loader2, X } from "lucide-react";
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import type { ImageRef, ToolCall } from "@/ipc/bindings";
import { parseAnsi, stripAnsi } from "@/lib/ansi";
import { cn } from "@/lib/cn";
import { formatDuration } from "@/lib/format";
import { TokenSpans, useHighlight } from "@/lib/shiki";
import { useImage } from "@/queries/hooks";
import { useUi } from "@/state/ui";
import { toolKey } from "../keys";
import { formatBytes, type OutputState, type ToolCtx } from "./data";

/** What a call without a result is doing; a permission card says it waits for approval instead. */
export const PendingText = createContext("执行中");

/** The spinner of a call without a result. */
export function PendingSpinner() {
  return <Spinner text={useContext(PendingText)} />;
}

/** What a call did, as a verb (the line reads 「读取 src/config.rs」); unknown tools keep their name. */
const VERB: Record<string, string> = {
  Bash: "运行",
  Read: "读取",
  Edit: "编辑",
  MultiEdit: "编辑",
  Write: "写入",
  NotebookEdit: "编辑",
  Grep: "搜索",
  Glob: "查找文件",
  WebFetch: "抓取",
  WebSearch: "搜索网页",
  TodoWrite: "任务清单",
  TaskCreate: "新建任务",
  TaskUpdate: "更新任务",
  TaskList: "任务列表",
};

/** A call's verb (「运行」「读取」), or its name for a tool without one. */
export const verbOf = (name: string) => VERB[name] ?? name;

/**
 * A tool call as one quiet line — icon, what it did, on what, and only a failure or what it waits for at the
 * right — that opens its details (output, diff) in a box below. Failed calls are red.
 */
export function Shell({
  nodeId,
  call,
  startMs,
  icon,
  name,
  summary,
  status,
  failed,
  children,
}: {
  nodeId: string;
  call: ToolCall;
  startMs: number | null;
  icon: ReactNode;
  /** Label for a tool without a verb of its own (MCP tools and the like). */
  name?: string;
  summary: ReactNode;
  /** Replaces the default status (nothing when done, 失败, or what a call without a result does). */
  status?: ReactNode;
  failed: boolean;
  children: ReactNode;
}) {
  const key = toolKey(nodeId, call.toolUseId);
  const open = useUi((s) => s.expanded[key] ?? false);
  const toggle = useUi((s) => s.toggleExpanded);
  const pendingText = useContext(PendingText);
  const result = call.result;
  const dur =
    result?.timestampMs != null && startMs != null && result.timestampMs >= startMs
      ? result.timestampMs - startMs
      : null;
  return (
    <div
      data-testid="tool-call"
      data-tool={call.name}
      data-failed={failed || undefined}
      className="flex w-full flex-col gap-1"
    >
      <button
        type="button"
        aria-expanded={open}
        // A link in the summary opens itself (the opener plugin takes `_blank` links) instead of toggling.
        onClick={(e) => (e.target as Element).closest("a[href]") || toggle(key)}
        className={cn(
          "group/tool flex min-h-7 w-full items-center gap-2 rounded-md text-left text-[13px]",
          failed ? "text-error" : "text-secondary hover:text-text",
        )}
      >
        <span className="flex shrink-0 items-center [&>svg]:size-[14px]">{icon}</span>
        <span className={cn("shrink-0", failed ? "" : "text-text/90")}>{VERB[call.name] ?? name ?? call.name}</span>
        <span className="flex min-w-0 flex-1 items-center gap-2">{summary}</span>
        {status ??
          (result == null ? (
            <span className="shrink-0 text-[12px] text-live-waiting">{pendingText}…</span>
          ) : failed ? (
            <span className="flex shrink-0 items-center gap-1 text-[12px] font-medium">
              <CircleX size={13} strokeWidth={1.8} />
              失败
            </span>
          ) : null)}
        {dur != null && (
          <span className="shrink-0 text-[11.5px] text-secondary/80">{formatDuration(dur, "detail")}</span>
        )}
        {open ? (
          <ChevronDown size={12} strokeWidth={1.8} className="shrink-0" aria-hidden />
        ) : (
          <ChevronRight
            size={12}
            strokeWidth={1.8}
            className="shrink-0 opacity-0 group-hover/tool:opacity-100"
            aria-hidden
          />
        )}
      </button>
      {open && (
        <div
          data-testid="tool-body"
          className={cn("ml-[22px] overflow-hidden rounded-lg border", failed ? "border-error/30" : "border-border")}
        >
          {children}
        </div>
      )}
    </div>
  );
}

export const Mono = ({ children, className }: { children: ReactNode; className?: string }) => (
  <code className={cn("min-w-0 truncate font-mono text-[12px]", className)}>{children}</code>
);

export function Label({ children }: { children: ReactNode }) {
  return <div className="px-3 pt-1.5 pb-0.5 text-[11px] text-secondary">{children}</div>;
}

export function Spinner({ text }: { text: string }) {
  return (
    <div className="flex items-center gap-2 px-3 py-2 text-[12px] text-secondary" role="status">
      <Loader2 size={13} className="animate-spin" aria-hidden />
      {text}
    </div>
  );
}

const HEAD = 30;
const TAIL = 20;

/** Footer with the "加载完整输出" button, source and loading / error state. */
function LoadBar({ out, dark }: { out: OutputState; dark: boolean }) {
  if (!out.canLoad && !out.source) return null;
  const tone = dark ? "bg-[#2a2b2f] text-[#a1a1a6]" : "bg-list text-secondary";
  return (
    <div className={cn("flex items-center gap-3 px-3 py-1.5 text-[11px]", tone)}>
      {out.canLoad && !out.loaded && (
        <button
          type="button"
          onClick={out.load}
          disabled={out.loading}
          className={cn(
            "flex items-center gap-1.5 rounded px-1.5 py-0.5 font-medium",
            dark ? "text-[#8ab4f8] hover:bg-white/10" : "text-accent hover:bg-selection",
          )}
        >
          {out.loading && <Loader2 size={11} className="animate-spin" aria-hidden />}
          加载完整输出{out.totalBytes > 0 ? `（共 ${formatBytes(out.totalBytes)}）` : ""}
        </button>
      )}
      {out.loaded && <span>已加载完整输出（{formatBytes(out.totalBytes)}）</span>}
      {out.capped && <span>超出 8 MB 的部分已截断</span>}
      {out.error && <span className={dark ? "text-[#ff8a80]" : "text-error"}>完整输出不可用</span>}
      {out.source && <span className="ml-auto font-mono">来源：{out.source}</span>}
    </div>
  );
}

function TermLines({ text }: { text: string }) {
  const [all, setAll] = useState(false);
  const lines = text.split("\n");
  const fold = !all && lines.length > HEAD + TAIL + 5;
  const shown = fold ? [...lines.slice(0, HEAD), null, ...lines.slice(-TAIL)] : lines;
  const hidden = lines.length - HEAD - TAIL;
  return (
    <>
      {shown.map((l, i) =>
        l === null ? (
          <button
            key="fold"
            type="button"
            onClick={() => setAll(true)}
            className="my-0.5 block w-full rounded bg-white/5 py-0.5 text-center text-[11px] text-[#a1a1a6] hover:bg-white/10"
          >
            ⋯ 中间 {hidden} 行已折叠 ⋯
          </button>
        ) : (
          <div key={i}>
            {parseAnsi(l).map((s, j) => (
              <span
                key={j}
                style={{
                  color: s.fg,
                  backgroundColor: s.bg,
                  fontWeight: s.bold ? 600 : undefined,
                  fontStyle: s.italic ? "italic" : undefined,
                  textDecoration: s.underline ? "underline" : undefined,
                  opacity: s.dim ? 0.65 : undefined,
                }}
              >
                {s.text}
              </span>
            ))}
            {l === "" && "\u200b"}
          </div>
        ),
      )}
    </>
  );
}

/** ANSI-aware terminal text; the middle of long output is folded ("⋯ 中间 N 行已折叠 ⋯"). `stderr` renders after stdout. */
export function Terminal({ text, stderr }: { text: string; stderr?: string }) {
  return (
    <pre
      data-testid="terminal"
      className="m-0 max-h-[28rem] overflow-auto bg-terminal px-3 py-2.5 font-mono text-[12px] leading-[1.55] whitespace-pre-wrap break-words text-[#e4e4e6]"
    >
      {text === "" && !stderr && <span className="text-[#a1a1a6]">（无输出）</span>}
      {text !== "" && <TermLines text={text} />}
      {stderr && (
        <>
          {text !== "" && <div className="mt-2 text-[11px] text-[#a1a1a6] select-none">stderr</div>}
          <TermLines text={stderr} />
        </>
      )}
    </pre>
  );
}

/** Output of a tool: terminal-styled or plain, plus the load-full-output bar. */
export function OutputView({
  out,
  terminal = false,
  empty = "（无输出）",
}: {
  out: OutputState;
  terminal?: boolean;
  empty?: string;
}) {
  return (
    <div data-testid="tool-output">
      {terminal ? (
        <Terminal text={out.text} />
      ) : (
        <pre className="m-0 max-h-80 overflow-auto bg-code px-3 py-2 font-mono text-[12px] leading-[1.55] whitespace-pre-wrap break-words">
          {stripAnsi(out.text) || empty}
        </pre>
      )}
      <LoadBar out={out} dark={terminal} />
    </div>
  );
}

const MAX_CODE_LINES = 3000;

/** Code with a line-number gutter, highlighted through the Shiki worker (plain until ready). */
export function CodeView({
  code,
  lang,
  startLine = 1,
  numbers,
  footer,
}: {
  code: string;
  lang: string | null;
  startLine?: number;
  /** Explicit line numbers (Read output); defaults to startLine + index. */
  numbers?: number[];
  footer?: ReactNode;
}) {
  const hl = useHighlight(code, lang);
  const lines = hl ?? code.split("\n").map((c) => [{ c }]);
  const shown = lines.slice(0, MAX_CODE_LINES);
  return (
    <div>
      <div
        data-testid="code-view"
        className="max-h-96 overflow-auto bg-code py-1.5 font-mono text-[12px] leading-[1.6]"
      >
        {shown.map((l, i) => (
          <div key={i} className="flex px-3">
            <span className="w-9 shrink-0 pr-2 text-right text-secondary/70 select-none">
              {numbers?.[i] ?? startLine + i}
            </span>
            <span className="min-w-0 whitespace-pre-wrap break-all">
              <TokenSpans tokens={l} />
              {l.length === 0 && "​"}
            </span>
          </div>
        ))}
        {lines.length > MAX_CODE_LINES && (
          <div className="px-3 pt-1 text-[11px] text-secondary">⋯ 其余 {lines.length - MAX_CODE_LINES} 行未显示 ⋯</div>
        )}
      </div>
      {footer}
    </div>
  );
}

export { LoadBar };

/** Lazy image thumbnail (loads when scrolled into view); click enlarges in a dialog. */
export function ImageThumb({ image, ctx }: { image: ImageRef; ctx: ToolCtx | null }) {
  const ref = useRef<HTMLButtonElement>(null);
  const [seen, setSeen] = useState(false);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || seen) return;
    if (typeof IntersectionObserver === "undefined") return setSeen(true);
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && setSeen(true), {
      rootMargin: "200px",
    });
    io.observe(el);
    return () => io.disconnect();
  }, [seen]);
  const q = useImage(seen && ctx ? { sessionId: ctx.sessionId, scope: ctx.scope, image } : null);
  const src = q.data ? `data:${q.data.mediaType};base64,${q.data.dataBase64}` : null;
  return (
    <>
      <button
        ref={ref}
        type="button"
        data-testid="image-thumb"
        aria-label="放大图片"
        onClick={() => src && setOpen(true)}
        className="flex h-24 min-w-24 max-w-56 items-center justify-center overflow-hidden rounded-md border border-border bg-code"
      >
        {src ? (
          <img src={src} alt="" className="max-h-full max-w-full object-contain" />
        ) : q.isError ? (
          <span className="px-2 text-[11px] text-secondary">图片不可用</span>
        ) : (
          <Loader2 size={14} className="animate-spin text-secondary" aria-hidden />
        )}
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="flex max-h-[90vh] w-auto max-w-[90vw] items-center justify-center p-2">
          <DialogTitle className="sr-only">图片预览</DialogTitle>
          <DialogDescription className="sr-only">{image.mediaType}</DialogDescription>
          {src && (
            <img data-testid="image-large" src={src} alt="" className="max-h-[85vh] max-w-[88vw] object-contain" />
          )}
          <button
            type="button"
            aria-label="关闭"
            onClick={() => setOpen(false)}
            className="absolute top-2 right-2 rounded bg-black/50 p-1 text-white"
          >
            <X size={14} />
          </button>
        </DialogContent>
      </Dialog>
    </>
  );
}

/**
 * An image where a prompt has it (`[Image #n]`, as Claude Code writes it), as a block in the line: hovered, a
 * preview above it; clicked, the image large.
 */
export function ImageMarker({
  image,
  label,
  ctx,
}: {
  image: ImageRef | undefined;
  label: string;
  ctx: ToolCtx | null;
}) {
  const [hover, setHover] = useState(false);
  const [open, setOpen] = useState(false);
  const q = useImage(image && ctx && (hover || open) ? { sessionId: ctx.sessionId, scope: ctx.scope, image } : null);
  const src = q.data ? `data:${q.data.mediaType};base64,${q.data.dataBase64}` : null;
  return (
    <span className="relative inline-block whitespace-nowrap">
      <button
        type="button"
        data-testid="prompt-image"
        aria-label={`放大${label}`}
        onPointerEnter={() => setHover(true)}
        onPointerLeave={() => setHover(false)}
        onClick={() => src && setOpen(true)}
        className="mx-0.5 inline-flex items-center gap-1 rounded-[5px] border border-border bg-ground px-1.5 align-[1px] text-[12.5px] leading-[20px] text-text hover:border-accent-border hover:text-accent"
      >
        <ImageIcon size={13} strokeWidth={1.8} className="text-secondary" aria-hidden />
        {label}
      </button>
      {hover && image && (
        <span
          role="tooltip"
          data-testid="prompt-image-preview"
          className="pointer-events-none absolute bottom-full left-0 z-30 mb-1.5 flex w-[240px] flex-col gap-1 rounded-[10px] border border-border bg-ground p-2 text-[11px] text-secondary shadow-[0_12px_32px_rgba(0,0,0,0.16),0_1px_3px_rgba(0,0,0,0.08)]"
        >
          {src ? (
            <img src={src} alt="" className="max-h-[150px] w-full rounded-md object-contain" />
          ) : q.isError ? (
            <span>图片不可用</span>
          ) : (
            <Loader2 size={14} className="animate-spin" aria-hidden />
          )}
          <span>{label}</span>
        </span>
      )}
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="flex max-h-[90vh] w-auto max-w-[90vw] items-center justify-center p-2">
          <DialogTitle className="sr-only">图片预览</DialogTitle>
          <DialogDescription className="sr-only">{label}</DialogDescription>
          {src && (
            <img data-testid="image-large" src={src} alt="" className="max-h-[85vh] max-w-[88vw] object-contain" />
          )}
        </DialogContent>
      </Dialog>
    </span>
  );
}

/** A row of thumbnails; also usable for user-prompt images (`body.images`). */
export function ImageRefs({ images, ctx }: { images: ImageRef[]; ctx: ToolCtx | null }) {
  if (images.length === 0) return null;
  return (
    <div className="flex flex-wrap gap-2 px-3 py-2">
      {images.map((im) => (
        <ImageThumb key={`${im.nodeId}:${im.ordinal}`} image={im} ctx={ctx} />
      ))}
    </div>
  );
}
