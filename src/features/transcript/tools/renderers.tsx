import { Check, Circle, CircleDot, FileText, Globe, Pencil, Search, SquareTerminal, Wrench } from "lucide-react";
import { useMemo, type ReactNode } from "react";
import { CopyButton } from "@/components/ui/copy-button";
import { cn } from "@/lib/cn";
import { diffStrings, joinDiffs, type DiffResult } from "@/lib/diff";
import { langFromPath } from "@/lib/shiki";
import type { ToolCall } from "../model";
import { num, str, toolKey, useExpanded, useOutput } from "./data";
import { CodeView, FailText, Label, LoadBar, Mono, OutputView, Shell, Spinner, Terminal } from "./parts";

export interface RenderProps {
  conn: string;
  call: ToolCall;
}

const ip = { size: 14, strokeWidth: 1.6, "aria-hidden": true } as const;
const isErr = (c: ToolCall) => c.result?.isError === true;

/* ───────────── Bash ───────────── */

export function BashView({ conn, call }: RenderProps) {
  const failed = isErr(call);
  const out = useOutput(conn, call);
  const r = call.result;
  // No stdout / stderr split from ccrpc: the status comes from the result text alone.
  const code = failed ? /^Exit code (\d+)/.exec(r?.content ?? "")?.[1] : undefined;
  const interrupted = !!r && /\[Request interrupted by user/.test(r.content);
  const status = interrupted ? (
    <span className="shrink-0 font-semibold">已中断</span>
  ) : code ? (
    <span className="shrink-0 font-semibold">退出码 {code}</span>
  ) : undefined;
  const desc = str(call.input.description);
  const command = str(call.input.command);
  return (
    <Shell
      conn={conn}
      call={call}
      icon={<SquareTerminal {...ip} />}
      status={status}
      summary={
        <>
          <Mono className="flex-1">
            <span className="text-secondary select-none">$ </span>
            {command.split("\n", 1)[0]}
          </Mono>
          {desc && <span className="hidden max-w-[40%] shrink-0 truncate text-secondary sm:inline">{desc}</span>}
        </>
      }
    >
      <div className="group/copy relative">
        <CopyButton text={command} label="复制命令" className="absolute top-1 right-1.5 z-10 bg-terminal text-[#a1a1a6]" />
        {command.includes("\n") && (
          <pre className="m-0 overflow-auto bg-terminal px-3 pt-2.5 font-mono text-[12px] leading-[1.55] whitespace-pre-wrap break-words text-[#e4e4e6]">
            <span className="text-[#a1a1a6] select-none">$ </span>
            {command}
          </pre>
        )}
        {r == null ? (
          call.unfinished ? null : <Spinner text="执行中" />
        ) : (
          <div data-testid="tool-result">
            <Terminal text={out.text} />
            <LoadBar out={out} dark />
          </div>
        )}
      </div>
    </Shell>
  );
}

/* ───────────── Edit / Write / Read ───────────── */

function DiffBody({ diff }: { diff: DiffResult }) {
  if (diff.rows.length === 0) return <div className="bg-code px-3 py-2 text-[12px] text-secondary">无改动</div>;
  return (
    <div data-testid="diff" className="max-h-[28rem] overflow-auto font-mono text-[12px] leading-[1.6]">
      {diff.rows.map((r, i) => {
        if (r.kind === "gap")
          return (
            <div key={i} className="bg-list px-3 text-center text-secondary select-none">
              ⋯
            </div>
          );
        const add = r.kind === "add";
        const del = r.kind === "del";
        return (
          <div
            key={i}
            data-diff={r.kind}
            className={cn("flex px-3", add && "bg-diff-add text-diff-add-text", del && "bg-diff-del text-diff-del-text", !add && !del && "bg-list text-secondary")}
          >
            <span className={cn("w-8 shrink-0 select-none", add && "text-diff-add-text/80", del && "text-diff-del-text/80")}>
              {add ? r.newNo : (r.oldNo ?? "")}
            </span>
            <span className="w-3.5 shrink-0 select-none">{add ? "+" : del ? "−" : ""}</span>
            <span className="min-w-0 whitespace-pre-wrap break-all">{r.text || "​"}</span>
          </div>
        );
      })}
    </div>
  );
}

/** Edit / MultiEdit diff from the input's old_string / new_string (ccrpc has no structured patch). */
export function editDiff(input: Record<string, unknown>): DiffResult {
  const edits = Array.isArray(input.edits) ? (input.edits as Record<string, unknown>[]) : [input];
  return joinDiffs(edits.map((e) => diffStrings(str(e.old_string), str(e.new_string))));
}

export function EditView({ conn, call }: RenderProps) {
  const failed = isErr(call);
  const open = useExpanded(toolKey(conn, call.id));
  const input = call.input;
  // The summary shows +/− counts, so the diff is computed even while collapsed.
  const diff = useMemo(() => editDiff(input), [input]);
  const replaceAll = input.replace_all === true || (Array.isArray(input.edits) && (input.edits as Record<string, unknown>[]).some((e) => e.replace_all === true));
  return (
    <Shell
      conn={conn}
      call={call}
      icon={<Pencil {...ip} />}
      name={call.name === "Edit" ? undefined : call.name}
      summary={
        <>
          <Mono className="flex-1">{str(input.file_path) || str(input.notebook_path)}</Mono>
          {replaceAll && <span className="shrink-0 rounded bg-black/5 px-1.5 text-[11px] dark:bg-white/10">全部替换</span>}
          {!failed && (
            <>
              <span className="shrink-0 font-semibold text-diff-add-text">+{diff.added}</span>
              <span className="shrink-0 font-semibold text-diff-del-text">−{diff.removed}</span>
            </>
          )}
        </>
      }
    >
      {failed && call.result && <FailText text={call.result.content} />}
      {open && <DiffBody diff={diff} />}
    </Shell>
  );
}

export function WriteView({ conn, call }: RenderProps) {
  const failed = isErr(call);
  const path = str(call.input.file_path);
  const content = str(call.input.content);
  const n = content === "" ? 0 : content.replace(/\n$/, "").split("\n").length;
  const created = /created/i.test(call.result?.content ?? "");
  return (
    <Shell
      conn={conn}
      call={call}
      icon={<Pencil {...ip} />}
      name="Write"
      summary={
        <>
          <Mono className="flex-1">{path}</Mono>
          <span className="shrink-0 text-secondary">（{created ? "新建" : "写入"}，{n} 行）</span>
        </>
      }
    >
      {failed && call.result && <FailText text={call.result.content} />}
      <CodeView code={content.replace(/\n$/, "")} lang={langFromPath(path)} />
    </Shell>
  );
}

/** Read output is `cat -n` style ("   12\tcode" or "   12→code"); split when every line matches. */
export function splitNumbered(text: string): { code: string; numbers: number[] } | null {
  const lines = text.replace(/\n$/, "").split("\n");
  const numbers: number[] = [];
  const code: string[] = [];
  for (const l of lines) {
    const m = /^\s*(\d+)[\t→](.*)$/.exec(l);
    if (!m) return null;
    numbers.push(Number(m[1]));
    code.push(m[2]!);
  }
  return lines.length ? { code: code.join("\n"), numbers } : null;
}

export function ReadView({ conn, call }: RenderProps) {
  const failed = isErr(call);
  const path = str(call.input.file_path);
  const offset = num(call.input.offset);
  const limit = num(call.input.limit);
  const range = offset != null && limit != null ? `L${offset}–${offset + limit - 1}` : offset != null ? `从 L${offset}` : limit != null ? `前 ${limit} 行` : "";
  const open = useExpanded(toolKey(conn, call.id));
  const out = useOutput(conn, call);
  const numbered = useMemo(() => (open ? splitNumbered(out.text) : null), [open, out.text]);
  return (
    <Shell
      conn={conn}
      call={call}
      icon={<FileText {...ip} />}
      name="Read"
      summary={
        <>
          <Mono className="flex-1">{path}</Mono>
          {range && <span className="shrink-0 text-secondary">{range}</span>}
        </>
      }
    >
      {!call.result ? (
        call.unfinished ? null : <Spinner text="执行中" />
      ) : failed ? (
        <FailText text={out.text} />
      ) : numbered ? (
        <div data-testid="tool-result">
          <CodeView code={numbered.code} numbers={numbered.numbers} lang={langFromPath(path)} footer={<LoadBar out={out} dark={false} />} />
        </div>
      ) : (
        <OutputView out={out} />
      )}
    </Shell>
  );
}

/* ───────────── TodoWrite ───────────── */

function StatusIcon({ status }: { status: string }) {
  if (status === "completed") return <Check size={13} strokeWidth={2.2} className="text-live" aria-label="已完成" />;
  if (status === "in_progress") return <CircleDot size={13} strokeWidth={1.8} className="text-accent" aria-label="进行中" />;
  return <Circle size={13} strokeWidth={1.6} className="text-secondary" aria-label="待办" />;
}

export function TodoView({ conn, call }: RenderProps) {
  const todos = Array.isArray(call.input.todos) ? (call.input.todos as Record<string, unknown>[]) : [];
  const items = todos.map((t) => ({ subject: str(t.content) || str(t.subject), status: str(t.status) || "pending" }));
  const done = items.filter((t) => t.status === "completed").length;
  const cur = items.find((t) => t.status === "in_progress");
  return (
    <Shell
      conn={conn}
      call={call}
      icon={<Check {...ip} />}
      name={call.name}
      summary={<Mono className="flex-1">{`${items.length} 项，${done} 项已完成${cur ? ` · 进行中：${cur.subject}` : ""}`}</Mono>}
    >
      {isErr(call) && call.result && <FailText text={call.result.content} />}
      <ul className="m-0 flex list-none flex-col gap-1 bg-ground px-3 py-2 text-[12px]" data-testid="checklist">
        {items.map((t, i) => (
          <li key={i} className="flex items-center gap-2">
            <StatusIcon status={t.status} />
            <span className={cn(t.status === "completed" && "text-secondary line-through", t.status === "in_progress" && "font-medium")}>{t.subject}</span>
          </li>
        ))}
      </ul>
    </Shell>
  );
}

/* ───────────── Grep / Glob / WebFetch / WebSearch ───────────── */

export function CompactView({ conn, call }: RenderProps) {
  const input = call.input;
  const out = useOutput(conn, call);
  let main = "";
  let tail = "";
  let icon: ReactNode = <Search {...ip} />;
  switch (call.name) {
    case "Grep":
      main = str(input.pattern);
      tail = [str(input.path) && `在 ${str(input.path)}`, str(input.glob), str(input.type)].filter(Boolean).join(" · ");
      break;
    case "Glob":
      main = str(input.pattern);
      tail = str(input.path) ? `在 ${str(input.path)}` : "";
      break;
    case "WebFetch":
      icon = <Globe {...ip} />;
      main = str(input.url);
      break;
    default:
      main = str(input.query);
  }
  const extras = Object.entries(input).filter(([k, v]) => !["pattern", "url", "query"].includes(k) && v != null && v !== "");
  return (
    <Shell
      conn={conn}
      call={call}
      icon={icon}
      name={call.name}
      summary={
        <>
          <Mono className="flex-1">{main}</Mono>
          {tail && <span className="hidden max-w-[40%] shrink-0 truncate text-secondary sm:inline">{tail}</span>}
        </>
      }
    >
      {extras.length > 0 && (
        <div className="flex flex-wrap gap-x-4 gap-y-0.5 bg-ground px-3 py-1.5 text-[11px] text-secondary">
          {extras.map(([k, v]) => (
            <span key={k}>
              {k}: <span className="font-mono text-text/80">{typeof v === "string" ? v : JSON.stringify(v)}</span>
            </span>
          ))}
        </div>
      )}
      {call.result ? <OutputView out={out} /> : call.unfinished ? null : <Spinner text="执行中" />}
    </Shell>
  );
}

/* ───────────── Generic ───────────── */

export function GenericView({ conn, call, summary }: RenderProps & { summary: string }) {
  const failed = isErr(call);
  const open = useExpanded(toolKey(conn, call.id));
  const out = useOutput(conn, call);
  const pretty = useMemo(() => (open ? JSON.stringify(call.input, null, 2) : ""), [open, call.input]);
  return (
    <Shell conn={conn} call={call} icon={<Wrench {...ip} />} name={call.name} summary={<Mono className="flex-1">{summary}</Mono>}>
      <Label>输入</Label>
      <CodeView code={pretty} lang="json" />
      {call.result ? (
        <>
          <Label>输出</Label>
          <OutputView out={out} terminal={failed} />
        </>
      ) : (
        !call.unfinished && <Spinner text="执行中" />
      )}
    </Shell>
  );
}
