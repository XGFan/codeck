import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { api } from "@/ipc";
import type { Diagnostics, NameCount } from "@/ipc/bindings";
import { queryKeys, useDiagnostics, useIndexStatus } from "@/queries";
import { errorText } from "@/lib/errors";
import { formatCount } from "@/lib/format";

const cmpVersion = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true });

/**
 * The line under 「Claude Code 版本」: how many versions, from which to which, and how many the table lists.
 * `unattributed`: the page shows failed lines or unknown items although no version has any (they come from
 * entries without a version, or Sessions with none).
 */
export function versionSummary(versions: string[], listed: number, unattributed: boolean): string {
  const vs = [...versions].sort(cmpVersion);
  if (vs.length === 0) return "没有记录到版本";
  const range = vs.length === 1 ? vs[0] : `从 ${vs[0]} 到 ${vs[vs.length - 1]}`;
  const rest = listed
    ? `；下表只列出有失败行或未知项的 ${listed} 个`
    : unattributed
      ? "；失败行与未知项都对应不到版本"
      : "，都没有失败行和未知项";
  return `共 ${vs.length} 个版本，${range}${rest}`;
}

function nameCountRows(title: string, rows: NameCount[]): string[] {
  if (rows.length === 0) return [];
  return [`## ${title}`, ...rows.map((r) => `- \`${r.name}\` × ${r.count}（${r.versions.join(", ")}）`), ""];
}

export function diagnosticsReport(d: Diagnostics): string {
  return [
    "# 格式兼容性报告",
    "",
    `- 扫描文件 ${formatCount(d.filesScanned)} 个，Session ${formatCount(d.sessions)} 个（空 ${formatCount(d.emptySessions)} 个）`,
    `- 解析失败 ${formatCount(d.failedLines)} 行，重复 uuid ${formatCount(d.duplicateUuids)} 个，孤立 Subagent ${formatCount(d.orphanSubagents)} 个`,
    "",
    ...nameCountRows("未知条目类型", d.unknownEntryTypes),
    ...nameCountRows("未知内容块类型", d.unknownBlockTypes),
    ...nameCountRows("未知系统子类型", d.unknownSystemSubtypes),
    ...nameCountRows("未知工具", d.unknownTools),
    ...nameCountRows("认不出的消息", d.unrecognizedMessages),
    "## 版本",
    ...d.versions.map(
      (v) => `- ${v.version}：${v.sessions} 个 Session，失败 ${v.failedLines} 行，未知项 ${v.unknownItems}`,
    ),
    "",
    ...(d.filesWithFailures.length
      ? ["## 解析失败的文件", ...d.filesWithFailures.map((f) => `- ${f.path}：${f.failedLines} 行，${f.firstError}`)]
      : []),
  ].join("\n");
}

type Tone = "neutral" | "error" | "warn";
const toneClass: Record<Tone, string> = {
  neutral: "border-border",
  error: "border-error/40 bg-error-bg text-error",
  warn: "border-claude/40 bg-claude/10 text-claude",
};

function Card({ label, value, tone, testId }: { label: string; value: string; tone: Tone; testId: string }) {
  return (
    <div data-testid={testId} className={`rounded-[10px] border px-3 py-2.5 ${toneClass[tone]}`}>
      <div className={`text-xs ${tone === "neutral" ? "text-secondary" : ""}`}>{label}</div>
      <div className="text-xl font-semibold tabular-nums">{value}</div>
    </div>
  );
}

function Table({ head, children, testId }: { head: string[]; children: React.ReactNode; testId?: string }) {
  return (
    <div data-testid={testId} className="overflow-hidden rounded-lg border border-border">
      <div
        className="grid gap-3 border-b border-border bg-list px-3 py-1.5 text-[11px] text-secondary"
        style={{ gridTemplateColumns: `2fr repeat(${head.length - 1}, 1fr)` }}
      >
        {head.map((h, i) => (
          <span key={h} className={i > 0 ? "text-right" : ""}>
            {h}
          </span>
        ))}
      </div>
      {children}
    </div>
  );
}

function NameCountSection({ title, rows, testId }: { title: string; rows: NameCount[]; testId: string }) {
  if (rows.length === 0) return null;
  return (
    <section data-testid={testId} className="flex flex-col gap-1.5">
      <h3 className="m-0 text-[13px] font-semibold">{title}</h3>
      <Table head={["名称", "出现次数", "版本"]}>
        {rows.map((r) => (
          <div
            key={r.name}
            className="grid gap-3 border-b border-border px-3 py-1.5 text-xs last:border-b-0"
            style={{ gridTemplateColumns: "2fr 1fr 1fr" }}
          >
            <code className="truncate font-mono">{r.name}</code>
            <span className="text-right tabular-nums">{formatCount(r.count)}</span>
            <span className="truncate text-right text-secondary" title={r.versions.join(", ")}>
              {r.versions.join(", ")}
            </span>
          </div>
        ))}
      </Table>
    </section>
  );
}

/** Body of the "格式兼容性" settings tab. */
export function DiagnosticsContent() {
  const qc = useQueryClient();
  const { data: d, error } = useDiagnostics();
  const { data: status } = useIndexStatus();
  const [copied, setCopied] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const rescan = useMutation({
    mutationFn: () => api.rebuildIndex(),
    onError: (e) => setActionError(errorText(e)),
    onSuccess: () => setActionError(null),
  });

  // The rescan finishes when the index returns to idle; refresh the counts then.
  const phase = status?.phase;
  const prev = useRef(phase);
  useEffect(() => {
    if (prev.current !== phase && phase === "idle") void qc.invalidateQueries({ queryKey: queryKeys.diagnostics });
    prev.current = phase;
  }, [phase, qc]);

  const copy = async (key: string, text: string) => {
    try {
      await api.copyText(text);
      setCopied(key);
      setTimeout(() => setCopied((c) => (c === key ? null : c)), 1500);
    } catch (e) {
      setActionError(errorText(e));
    }
  };

  if (error) return <div className="p-6 text-xs text-error">诊断数据读取失败：{errorText(error)}</div>;
  if (!d) return <div className="p-6 text-xs text-secondary">正在读取诊断数据…</div>;

  const busy = phase === "scanning" || phase === "indexingText" || rescan.isPending;
  const flagged = d.versions
    .filter((v) => v.failedLines > 0 || v.unknownItems > 0)
    .sort((a, b) => cmpVersion(b.version, a.version));
  const unknownTypes =
    d.unknownEntryTypes.length +
    d.unknownBlockTypes.length +
    d.unknownSystemSubtypes.length +
    d.unrecognizedMessages.length;

  return (
    <div data-testid="diagnostics" className="flex flex-col gap-5">
      <div className="flex flex-wrap items-center gap-3">
        <div className="min-w-60 flex-1">
          <h2 className="m-0 text-[15px] font-semibold">格式兼容性</h2>
          <div className="text-xs text-secondary">
            {formatCount(d.filesScanned)} 个文件 · {formatCount(d.sessions)} 个 Session（空{" "}
            {formatCount(d.emptySessions)} 个）
          </div>
        </div>
        <Button variant="outline" disabled={busy} onClick={() => rescan.mutate()}>
          {busy ? "正在扫描…" : "重新扫描"}
        </Button>
        <Button variant="outline" onClick={() => void copy("report", diagnosticsReport(d))}>
          {copied === "report" ? "已复制" : "复制报告"}
        </Button>
      </div>
      {actionError && (
        <div role="alert" className="text-xs text-error">
          {actionError}
        </div>
      )}

      <div className="grid grid-cols-2 gap-2.5 md:grid-cols-4">
        <Card testId="diag-card-parsed" label="已扫描文件" tone="neutral" value={formatCount(d.filesScanned)} />
        <Card
          testId="diag-card-failed"
          label="解析失败"
          tone={d.failedLines ? "error" : "neutral"}
          value={`${formatCount(d.failedLines)} 行`}
        />
        <Card
          testId="diag-card-types"
          label="未知类型"
          tone={unknownTypes ? "warn" : "neutral"}
          value={`${unknownTypes} 种`}
        />
        <Card
          testId="diag-card-tools"
          label="未知工具"
          tone={d.unknownTools.length ? "warn" : "neutral"}
          value={`${d.unknownTools.length} 个`}
        />
      </div>

      <NameCountSection testId="diag-entry-types" title="未知条目类型" rows={d.unknownEntryTypes} />
      <NameCountSection testId="diag-block-types" title="未知内容块类型" rows={d.unknownBlockTypes} />
      <NameCountSection testId="diag-subtypes" title="未知系统子类型" rows={d.unknownSystemSubtypes} />
      <NameCountSection testId="diag-tools" title="未知工具 · 以通用视图显示" rows={d.unknownTools} />
      <NameCountSection
        testId="diag-messages"
        title="认不出的消息 · 显示为你的消息或隐藏"
        rows={d.unrecognizedMessages}
      />

      <section data-testid="diag-versions" className="flex flex-col gap-1.5">
        <h3 className="m-0 text-[13px] font-semibold">Claude Code 版本</h3>
        <div data-testid="diag-versions-summary" className="text-xs text-secondary">
          {versionSummary(
            d.versions.map((v) => v.version),
            flagged.length,
            d.failedLines > 0 || unknownTypes > 0 || d.unknownTools.length > 0,
          )}
        </div>
        {flagged.length > 0 && (
          <Table head={["版本", "Session 数", "失败行", "未知项", "最后出现"]}>
            {flagged.map((v) => (
              <div
                key={v.version}
                data-testid="diag-version-row"
                className="grid gap-3 border-b border-border px-3 py-1.5 text-xs tabular-nums last:border-b-0"
                style={{ gridTemplateColumns: "2fr repeat(4, 1fr)" }}
              >
                <code className="font-mono">{v.version}</code>
                <span className="text-right">{formatCount(v.sessions)}</span>
                <span className={`text-right ${v.failedLines ? "text-error" : ""}`}>{formatCount(v.failedLines)}</span>
                <span className="text-right">{formatCount(v.unknownItems)}</span>
                <span className="text-right text-secondary">{new Date(v.lastSeenMs).toLocaleDateString("zh-CN")}</span>
              </div>
            ))}
          </Table>
        )}
      </section>

      <section data-testid="diag-failures" className="flex flex-col gap-1.5">
        <h3 className="m-0 text-[13px] font-semibold">解析失败的文件</h3>
        {d.filesWithFailures.length === 0 ? (
          <div className="text-xs text-secondary">没有解析失败的文件</div>
        ) : (
          <div className="overflow-hidden rounded-lg border border-border">
            {d.filesWithFailures.map((f) => (
              <div
                key={f.path}
                className="flex flex-wrap items-center gap-3 border-b border-border px-3 py-1.5 text-xs last:border-b-0"
              >
                <code className="min-w-60 flex-[3] truncate font-mono text-[11px]" title={f.path}>
                  {f.path}
                </code>
                <span className="w-16 text-right tabular-nums">{formatCount(f.failedLines)} 行</span>
                <span className="min-w-40 flex-[2] text-error">{f.firstError}</span>
                <button type="button" className="text-accent hover:underline" onClick={() => void copy(f.path, f.path)}>
                  {copied === f.path ? "已复制" : "复制路径"}
                </button>
              </div>
            ))}
          </div>
        )}
      </section>

      <div data-testid="diag-integrity" className="flex gap-6 text-xs text-secondary">
        <span>
          重复 uuid <b className="tabular-nums text-text">{formatCount(d.duplicateUuids)}</b>
        </span>
        <span>
          孤立 Subagent <b className="tabular-nums text-text">{formatCount(d.orphanSubagents)}</b>
        </span>
      </div>
    </div>
  );
}
