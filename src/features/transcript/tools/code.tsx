import { CircleX, CodeXml } from "lucide-react";
import type { ReactNode } from "react";
import { CopyButton } from "@/components/ui/copy-button";
import type { ToolCall } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { describeCalls } from "../grouping";
import { Anchor } from "../nodes/Anchor";
import { parseJson } from "../util";
import { CodeView, Label, Mono, Shell, Spinner } from "./parts";
import type { RenderProps } from "./renderers";

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/**
 * Claude's code (Code Mode's `REPL`) as one card, the calls it made inside: collapsed, what it is for, what it called
 * and how long it ran; opened, the code, the calls (each opens as usual) and what it returned or threw. Without a code
 * run (a permission request for it) only the code.
 */
export function CodeRunView({
  nodeId,
  call,
  startMs,
  renderCall,
}: RenderProps & { renderCall: (call: ToolCall) => ReactNode }) {
  const input = parseJson(call.inputJson) ?? {};
  const code = str(input.code);
  const description = str(input.description);
  const run = call.code;
  const calls = run?.calls ?? [];
  const outcome = run?.outcome ?? null;
  const threw = outcome?.isError === true;
  const failedCalls = calls.filter((c) => c.result?.isError === true).length;
  const status =
    call.result == null ? undefined : threw ? (
      <span className="shrink-0 text-[12px] font-semibold">出错</span>
    ) : run && !outcome ? (
      <span className="shrink-0 text-[12px] text-live-waiting">执行中…</span>
    ) : failedCalls > 0 ? (
      <span className="flex shrink-0 items-center gap-1 text-[12px] text-error">
        <CircleX size={12} strokeWidth={1.8} aria-hidden />
        {failedCalls} 失败
      </span>
    ) : (
      false
    );
  return (
    <Shell
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={threw}
      icon={<CodeXml size={14} strokeWidth={1.6} aria-hidden />}
      status={status}
      durationMs={run ? (outcome?.durationMs ?? null) : undefined}
      summary={
        <>
          {description ? (
            <span className="min-w-0 flex-1 truncate">{description}</span>
          ) : (
            <Mono className="flex-1">{code.split("\n", 1)[0]}</Mono>
          )}
          {calls.length > 0 && (
            <span className="hidden max-w-[40%] shrink-0 truncate text-secondary sm:inline">
              {describeCalls(calls).text}
            </span>
          )}
        </>
      }
    >
      <div className="group/copy flex items-center px-3 pt-1 text-[11px] text-secondary">
        代码
        <CopyButton text={code} label="复制代码" className="ml-auto" />
      </div>
      <CodeView code={code} lang="javascript" />
      {calls.length > 0 && (
        <>
          <Label>调用的工具（{calls.length}）</Label>
          <div data-testid="code-calls" className="flex flex-col gap-0.5 px-3 pb-1.5">
            {calls.map((c) => (
              <Anchor key={c.toolUseId} nodeId={nodeId} toolUseId={c.toolUseId} className="w-full">
                {renderCall(c)}
              </Anchor>
            ))}
          </div>
        </>
      )}
      {outcome ? (
        <>
          {outcome.log && (
            <>
              <Label>日志</Label>
              <pre className="m-0 max-h-40 overflow-auto bg-code px-3 py-2 font-mono text-[12px] leading-[1.55] whitespace-pre-wrap break-words text-secondary">
                {outcome.log}
              </pre>
            </>
          )}
          <Label>{threw ? "错误" : "返回值"}</Label>
          <pre
            data-testid="code-outcome"
            className={cn(
              "m-0 max-h-80 overflow-auto bg-code px-3 py-2 font-mono text-[12px] leading-[1.55] whitespace-pre-wrap break-words",
              threw && "text-error",
            )}
          >
            {outcome.text || "（无返回值）"}
            {outcome.truncated && "\n…（已截断）"}
          </pre>
        </>
      ) : (
        run && call.result != null && <Spinner text="等待返回值" />
      )}
    </Shell>
  );
}
