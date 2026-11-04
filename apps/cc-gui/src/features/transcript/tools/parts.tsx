import { Check, ChevronDown, ChevronRight, CircleX, Loader2 } from "lucide-react";
import { useState, type ReactNode } from "react";
import { parseAnsi, stripAnsi } from "@/lib/ansi";
import { cn } from "@/lib/cn";
import { TokenSpans, useHighlight } from "@/lib/shiki";
import { useUi } from "@/state/ui";
import type { ToolCall } from "../model";
import { formatBytes, toolKey, useExpanded, type OutputState } from "./data";

/** 执行中 / 未完成 / 失败 / 完成. */
export function callStatus(call: ToolCall): "running" | "unfinished" | "failed" | "done" {
  if (!call.result) return call.unfinished ? "unfinished" : "running";
  return call.result.isError ? "failed" : "done";
}

export function StatusBadge({ call }: { call: ToolCall }) {
  switch (callStatus(call)) {
    case "running":
      return (
        <span className="flex shrink-0 items-center gap-1 text-secondary" role="status">
          <Loader2 size={12} className="animate-spin" aria-hidden />
          执行中
        </span>
      );
    case "unfinished":
      return <span className="shrink-0 text-secondary">未完成</span>;
    case "failed":
      return (
        <span className="flex shrink-0 items-center gap-1 font-semibold">
          <CircleX size={13} strokeWidth={1.8} />
          失败
        </span>
      );
    case "done":
      return (
        <span className="flex shrink-0 items-center gap-1 text-live">
          <Check size={12} strokeWidth={2} />
          完成
        </span>
      );
  }
}

/** Collapsible frame: one-line header (summary) and an expandable body; failed calls are red. */
export function Shell({
  conn,
  call,
  icon,
  name,
  summary,
  status,
  children,
}: {
  conn: string;
  call: ToolCall;
  icon: ReactNode;
  /** Tool name label; omitted for Bash / Edit where the icon says it. */
  name?: string;
  summary: ReactNode;
  /** Replaces the default status badge. */
  status?: ReactNode;
  children: ReactNode;
}) {
  const key = toolKey(conn, call.id);
  const open = useExpanded(key);
  const failed = call.result?.isError === true;
  return (
    <div
      data-testid="tool-call"
      data-tool={call.name}
      data-status={callStatus(call)}
      className={cn("w-full overflow-hidden rounded-lg border", failed ? "border-error/30" : "border-border")}
    >
      <button
        type="button"
        aria-expanded={open}
        onClick={() => useUi.getState().toggle(key, !open)}
        className={cn(
          "flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-[12px]",
          open && "border-b",
          failed ? "border-error/30 bg-error-bg text-error" : "border-border bg-list text-text/80",
        )}
      >
        {open ? <ChevronDown size={12} strokeWidth={1.8} /> : <ChevronRight size={12} strokeWidth={1.8} />}
        {icon}
        {name && <span className="shrink-0 font-semibold">{name}</span>}
        <span className="flex min-w-0 flex-1 items-center gap-2">{summary}</span>
        {status ?? <StatusBadge call={call} />}
      </button>
      {open && <div data-testid="tool-body">{children}</div>}
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

export function FailText({ text }: { text: string }) {
  return <div className="border-b border-error/30 bg-error-bg px-3 py-1.5 text-[12px] whitespace-pre-wrap text-error">{stripAnsi(text)}</div>;
}

const HEAD = 30;
const TAIL = 20;

/** Footer with the "加载完整输出" button, source and loading / error state. */
export function LoadBar({ out, dark }: { out: OutputState; dark: boolean }) {
  if (!out.canLoad) return null;
  const tone = dark ? "bg-[#2a2b2f] text-[#a1a1a6]" : "bg-list text-secondary";
  return (
    <div className={cn("flex items-center gap-3 px-3 py-1.5 text-[11px]", tone)}>
      {!out.loaded && (
        <button
          type="button"
          data-testid="load-full"
          onClick={out.load}
          disabled={out.loading}
          className={cn("flex items-center gap-1.5 rounded px-1.5 py-0.5 font-medium", dark ? "text-[#8ab4f8] hover:bg-white/10" : "text-accent hover:bg-selection")}
        >
          {out.loading && <Loader2 size={11} className="animate-spin" aria-hidden />}
          加载完整输出{out.totalBytes > 0 ? `（共 ${formatBytes(out.totalBytes)}）` : ""}
        </button>
      )}
      {out.loaded && <span>已加载完整输出</span>}
      {out.capped && <span>超出 100 KB 的部分已截断</span>}
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
            {l === "" && "​"}
          </div>
        ),
      )}
    </>
  );
}

/** ANSI-aware terminal text; the middle of long output is folded ("⋯ 中间 N 行已折叠 ⋯"). */
export function Terminal({ text }: { text: string }) {
  return (
    <pre
      data-testid="terminal"
      className="m-0 max-h-[28rem] overflow-auto bg-terminal px-3 py-2.5 font-mono text-[12px] leading-[1.55] whitespace-pre-wrap break-words text-[#e4e4e6]"
    >
      {text === "" ? <span className="text-[#a1a1a6]">（无输出）</span> : <TermLines text={text} />}
    </pre>
  );
}

/** Output of a tool: terminal-styled or plain, plus the load-full-output bar. */
export function OutputView({ out, terminal = false, empty = "（无输出）" }: { out: OutputState; terminal?: boolean; empty?: string }) {
  return (
    <div data-testid="tool-result">
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
  numbers,
  footer,
}: {
  code: string;
  lang: string | null;
  /** Explicit line numbers (Read output); defaults to 1 + index. */
  numbers?: number[];
  footer?: ReactNode;
}) {
  const hl = useHighlight(code, lang);
  const lines = hl ?? code.split("\n").map((c) => [{ c }]);
  const shown = lines.slice(0, MAX_CODE_LINES);
  return (
    <div>
      <div data-testid="code-view" className="max-h-96 overflow-auto bg-code py-1.5 font-mono text-[12px] leading-[1.6]">
        {shown.map((l, i) => (
          <div key={i} className="flex px-3">
            <span className="w-9 shrink-0 pr-2 text-right text-secondary/70 select-none">{numbers?.[i] ?? 1 + i}</span>
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
