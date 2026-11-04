import { useUi } from "@/state/ui";
import { Check, Circle, CircleDot, FileText, Globe, Pencil, Search, SquareTerminal, Wrench } from "lucide-react";
import { useMemo, type ReactNode } from "react";

import { CopyButton } from "@/components/ui/copy-button";
import type { ToolCall } from "@/ipc/bindings";
import { diffStrings, joinDiffs, rowsFromHunks, type DiffResult, type Hunk } from "@/lib/diff";
import { langFromPath } from "@/lib/shiki";
import { cn } from "@/lib/cn";
import { prettyJson } from "../util";
import { parseJson, useFullInput, useOutput, type ToolCtx } from "./data";
import { CodeView, ImageRefs, Label, LoadBar, Mono, OutputView, PendingSpinner, Shell, Spinner, Terminal } from "./parts";

export interface RenderProps {
  nodeId: string;
  call: ToolCall;
  startMs: number | null;
  ctx: ToolCtx | null;
}

const ip = { size: 14, strokeWidth: 1.6, "aria-hidden": true } as const;
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const num = (v: unknown): number | null => (typeof v === "number" ? v : null);
const isErr = (c: ToolCall) => c.result?.isError === true;

/* ───────────── Bash ───────────── */

interface BashExtra {
  stdout?: string;
  stderr?: string;
  interrupted?: boolean;
  returnCodeInterpretation?: string;
}

export function BashView({ nodeId, call, startMs, ctx }: RenderProps) {
  const failed = isErr(call);
  const input = parseJson(call.inputJson) ?? {};
  const extra = parseJson<BashExtra>(call.result?.extraJson);
  const r = call.result;
  const split = !!extra && !r?.truncated && !r?.persisted && (typeof extra.stdout === "string" || typeof extra.stderr === "string");
  const out = useOutput(call, ctx, split ? (extra!.stdout ?? "") : undefined);
  const stderr = split && !out.loaded ? (extra!.stderr ?? "") : "";
  const code = /(\d+)/.exec(extra?.returnCodeInterpretation ?? "")?.[1];
  const status =
    r == null ? undefined : extra?.interrupted ? (
      <span className="font-semibold">已中断</span>
    ) : failed ? (
      <span className="font-semibold">{code ? `退出码 ${code}` : "失败"}</span>
    ) : undefined;
  const desc = str(input.description);
  const command = str(input.command);
  return (
    <Shell
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={failed}
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
        <PendingSpinner />
      ) : (
        <div data-testid="tool-output">
          <Terminal text={out.text} stderr={stderr} />
          <LoadBar out={out} dark />
        </div>
      )}
      </div>
      <ImageRefs images={r?.images ?? []} ctx={ctx} />
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

function editDiff(input: Record<string, unknown>, extraJson: string | null | undefined): DiffResult {
  const ex = parseJson<{ structuredPatch?: Hunk[] }>(extraJson);
  if (ex?.structuredPatch?.length) return rowsFromHunks(ex.structuredPatch);
  const edits = Array.isArray(input.edits) ? (input.edits as Record<string, unknown>[]) : [input];
  return joinDiffs(edits.map((e) => diffStrings(str(e.old_string), str(e.new_string))));
}

export function EditView({ nodeId, call, startMs, ctx }: RenderProps) {
  const failed = isErr(call);
  const key = `tool:${nodeId}|${call.toolUseId}`;
  const expanded = useOpenState(key);
  const { input, loading, truncated } = useFullInput(call, ctx, expanded);
  const hasPatch = !!parseJson<{ structuredPatch?: unknown[] }>(call.result?.extraJson)?.structuredPatch?.length;
  const ready = hasPatch || !call.inputTruncated || !loading;
  const diff = useMemo(() => (ready && (hasPatch || !truncated) ? editDiff(input, call.result?.extraJson) : null), [ready, hasPatch, truncated, input, call.result?.extraJson]);
  const replaceAll = input.replace_all === true || (Array.isArray(input.edits) && (input.edits as Record<string, unknown>[]).some((e) => e.replace_all === true));
  return (
    <Shell
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={failed}
      icon={<Pencil {...ip} />}
      name={call.name === "Edit" ? undefined : call.name}
      summary={
        <>
          <Mono className="flex-1">{str(input.file_path) || str(input.notebook_path)}</Mono>
          {replaceAll && <span className="shrink-0 rounded bg-black/5 px-1.5 text-[11px] dark:bg-white/10">全部替换</span>}
          {diff && !failed && (
            <>
              <span className="shrink-0 font-semibold text-diff-add-text">+{diff.added}</span>
              <span className="shrink-0 font-semibold text-diff-del-text">−{diff.removed}</span>
            </>
          )}
        </>
      }
    >
      {failed && call.result && <FailText text={call.result.text} />}
      {diff ? <DiffBody diff={diff} /> : <Spinner text="正在加载完整输入" />}
    </Shell>
  );
}

function FailText({ text }: { text: string }) {
  return <div className="border-b border-error/30 bg-error-bg px-3 py-1.5 text-[12px] whitespace-pre-wrap text-error">{text}</div>;
}

export function WriteView({ nodeId, call, startMs, ctx }: RenderProps) {
  const failed = isErr(call);
  const expanded = useOpenState(`tool:${nodeId}|${call.toolUseId}`);
  const { input, loading, truncated } = useFullInput(call, ctx, expanded);
  const path = str(input.file_path);
  const content = str(input.content);
  const n = truncated ? null : content === "" ? 0 : content.replace(/\n$/, "").split("\n").length;
  const created = call.result?.text.includes("created");
  const note = n == null ? "" : `（${created ? "新建" : "写入"}，${n} 行）`;
  return (
    <Shell
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={failed}
      icon={<Pencil {...ip} />}
      name="Write"
      summary={
        <>
          <Mono className="flex-1">{path}</Mono>
          {note && <span className="shrink-0 text-secondary">{note}</span>}
        </>
      }
    >
      {failed && call.result && <FailText text={call.result.text} />}
      {loading ? <Spinner text="正在加载完整输入" /> : <CodeView code={content.replace(/\n$/, "")} lang={langFromPath(path)} />}
    </Shell>
  );
}

/** Read output is `cat -n` style ("   12\tcode"); split into numbers + code when every line matches. */
function splitNumbered(text: string): { code: string; numbers: number[] } | null {
  const lines = text.replace(/\n$/, "").split("\n");
  const numbers: number[] = [];
  const code: string[] = [];
  for (const l of lines) {
    const m = /^\s*(\d+)\t(.*)$/.exec(l);
    if (!m) return null;
    numbers.push(Number(m[1]));
    code.push(m[2]!);
  }
  return lines.length ? { code: code.join("\n"), numbers } : null;
}

export function ReadView({ nodeId, call, startMs, ctx }: RenderProps) {
  const failed = isErr(call);
  const input = parseJson(call.inputJson) ?? {};
  const path = str(input.file_path);
  const offset = num(input.offset);
  const limit = num(input.limit);
  const range = offset != null && limit != null ? `L${offset}–${offset + limit - 1}` : offset != null ? `从 L${offset}` : limit != null ? `前 ${limit} 行` : "";
  const out = useOutput(call, ctx);
  const imgOnly = (call.result?.images.length ?? 0) > 0 && /^\[image\]\s*$/.test(out.text);
  const numbered = useMemo(() => splitNumbered(out.text), [out.text]);
  return (
    <Shell
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={failed}
      icon={<FileText {...ip} />}
      name="Read"
      summary={
        <>
          <Mono className="flex-1">{path}</Mono>
          {range && <span className="shrink-0 text-secondary">{range}</span>}
        </>
      }
    >
      {failed ? (
        <FailText text={out.text} />
      ) : (
        !imgOnly &&
        (numbered ? (
          <CodeView code={numbered.code} numbers={numbered.numbers} lang={langFromPath(path)} footer={<LoadBar out={out} dark={false} />} />
        ) : (
          <OutputView out={out} />
        ))
      )}
      <ImageRefs images={call.result?.images ?? []} ctx={ctx} />
    </Shell>
  );
}

/* ───────────── Tasks / TodoWrite ───────────── */

interface TaskItem {
  subject: string;
  status: string;
  id?: string;
}

function StatusIcon({ status }: { status: string }) {
  if (status === "completed") return <Check size={13} strokeWidth={2.2} className="text-live" aria-label="已完成" />;
  if (status === "in_progress") return <CircleDot size={13} strokeWidth={1.8} className="text-accent" aria-label="进行中" />;
  return <Circle size={13} strokeWidth={1.6} className="text-secondary" aria-label="待办" />;
}

const STATUS_TEXT: Record<string, string> = { pending: "待办", in_progress: "进行中", completed: "已完成", deleted: "已删除" };

function Checklist({ items }: { items: TaskItem[] }) {
  return (
    <ul className="m-0 flex list-none flex-col gap-1 bg-ground px-3 py-2 text-[12px]" data-testid="checklist">
      {items.map((t, i) => (
        <li key={i} className="flex items-center gap-2">
          <StatusIcon status={t.status} />
          <span className={cn(t.status === "completed" && "text-secondary line-through", t.status === "in_progress" && "font-medium")}>{t.subject}</span>
        </li>
      ))}
    </ul>
  );
}

export function TaskView({ nodeId, call, startMs, ctx }: RenderProps) {
  const failed = isErr(call);
  const input = parseJson(call.inputJson) ?? {};
  const extra = parseJson<{ task?: { id?: string; subject?: string }; statusChange?: { from?: string; to?: string } }>(call.result?.extraJson);
  const out = useOutput(call, ctx);
  let items: TaskItem[] = [];
  let summary = "";
  if (call.name === "TodoWrite") {
    const todos = Array.isArray(input.todos) ? (input.todos as Record<string, unknown>[]) : [];
    items = todos.map((t) => ({ subject: str(t.content) || str(t.subject), status: str(t.status) || "pending" }));
    const done = items.filter((t) => t.status === "completed").length;
    const cur = items.find((t) => t.status === "in_progress");
    summary = `${items.length} 项，${done} 项已完成${cur ? ` · 进行中：${cur.subject}` : ""}`;
  } else if (call.name === "TaskCreate") {
    const subject = str(input.subject) || extra?.task?.subject || "";
    items = [{ subject, status: "pending", id: extra?.task?.id }];
    summary = `新建任务：${subject}`;
  } else if (call.name === "TaskUpdate") {
    const id = str(input.taskId) || extra?.task?.id || "";
    const to = extra?.statusChange?.to ?? str(input.status);
    const from = extra?.statusChange?.from;
    const subject = str(input.subject);
    items = [{ subject: subject || `任务 #${id}`, status: to || "pending", id }];
    summary = `任务 #${id}${to ? `：${from ? `${STATUS_TEXT[from] ?? from} → ` : ""}${STATUS_TEXT[to] ?? to}` : ""}${subject ? ` · ${subject}` : ""}`;
  } else {
    summary = "任务列表";
  }
  return (
    <Shell
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={failed}
      icon={<Check {...ip} />}
      name={call.name}
      summary={<Mono className="flex-1">{summary}</Mono>}
    >
      {call.name === "TaskList" || failed ? <OutputView out={out} /> : <Checklist items={items} />}
    </Shell>
  );
}

/* ───────────── Grep / Glob / WebFetch / WebSearch ───────────── */

export function CompactView({ nodeId, call, startMs, ctx }: RenderProps) {
  const failed = isErr(call);
  const input = parseJson(call.inputJson) ?? {};
  const out = useOutput(call, ctx);
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
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={failed}
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
      {call.result ? <OutputView out={out} /> : <PendingSpinner />}
    </Shell>
  );
}

/* ───────────── Generic ───────────── */

export function GenericView({ nodeId, call, startMs, ctx, summary }: RenderProps & { summary: string }) {
  const failed = isErr(call);
  const expanded = useOpenState(`tool:${nodeId}|${call.toolUseId}`);
  const { input, loading, truncated } = useFullInput(call, ctx, expanded);
  const out = useOutput(call, ctx);
  const pretty = useMemo(() => (expanded ? prettyJson(JSON.stringify(input)) : ""), [expanded, input]);
  return (
    <Shell
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={failed}
      icon={<Wrench {...ip} />}
      name={call.name}
      summary={<Mono className="flex-1">{summary}</Mono>}
    >
      <Label>输入</Label>
      {loading ? (
        <Spinner text="正在加载完整输入" />
      ) : (
        <CodeView code={pretty} lang="json" startLine={1} footer={truncated ? <div className="bg-code px-3 pb-1.5 text-[11px] text-secondary">… 输入已截断</div> : null} />
      )}
      {call.result && (
        <>
          <Label>输出</Label>
          <OutputView out={out} terminal={failed} />
          <ImageRefs images={call.result.images} ctx={ctx} />
        </>
      )}
    </Shell>
  );
}

function useOpenState(key: string): boolean {
  return useUi((s) => s.expanded[key] ?? false);
}
