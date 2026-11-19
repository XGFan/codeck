import { CodeXml } from "lucide-react";
import { CopyButton } from "@/components/ui/copy-button";
import { cn } from "@/lib/cn";
import { parseJson } from "../util";
import { CodeView, Label, Mono, Shell, Spinner } from "./parts";
import type { RenderProps } from "./renderers";

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** The code of a `REPL` call, with its copy button. */
export function CodeBlock({ code }: { code: string }) {
  return (
    <>
      <div className="group/copy flex items-center px-3 pt-1 text-[11px] text-secondary">
        代码
        <CopyButton text={code} label="复制代码" className="ml-auto" />
      </div>
      <CodeView code={code} lang="javascript" />
    </>
  );
}

/**
 * Claude's code (Code Mode's `REPL`) as a light line before the calls it made, which have rows of their own: what it
 * is for (else its first line), how long it is and how long it ran; opened, the code and what it returned or threw.
 * Without a code run (a permission request for it) only the code.
 */
export function CodeRunView({ nodeId, call, startMs }: RenderProps) {
  const input = parseJson(call.inputJson) ?? {};
  const code = str(input.code);
  const description = str(input.description);
  const run = call.code;
  const outcome = run?.outcome ?? null;
  const threw = outcome?.isError === true;
  const lines = code.split("\n").length;
  const status =
    call.result == null ? undefined : threw ? (
      <span className="shrink-0 font-semibold">出错</span>
    ) : run && !outcome ? (
      <span className="shrink-0 text-live-waiting">执行中…</span>
    ) : (
      false
    );
  return (
    <Shell
      nodeId={nodeId}
      call={call}
      startMs={startMs}
      failed={threw}
      quiet
      icon={<CodeXml aria-hidden />}
      status={status}
      durationMs={run ? (outcome?.durationMs ?? null) : undefined}
      summary={
        <>
          {description ? (
            <span className="min-w-0 truncate">{description}</span>
          ) : (
            <Mono className="text-[11.5px]">{code.split("\n", 1)[0]}</Mono>
          )}
          <span className="shrink-0">· {lines} 行</span>
        </>
      }
    >
      <CodeBlock code={code} />
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
