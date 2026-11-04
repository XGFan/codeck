import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { AnalyzerInfo } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { useDrive } from "./store";

function Row({ label, children, testId }: { label: string; children: React.ReactNode; testId?: string }) {
  return (
    <div data-testid={testId} className="grid grid-cols-[140px_1fr] items-baseline gap-4 py-3 text-[14px]">
      <div className="text-secondary">{label}</div>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

/** Settings › 驾驶: ccrpc's path, `ccrpc doctor`, the analyzer, the log directory. Asks for `driveInfo` when shown. */
export function DriveSettings() {
  const info = useDrive((s) => s.info);
  const infoError = useDrive((s) => s.infoError);
  const lsError = useDrive((s) => s.lsError);
  const loadInfo = useDrive((s) => s.loadInfo);
  const retry = useDrive((s) => s.retry);

  useEffect(() => {
    void loadInfo();
  }, [loadInfo]);

  const stopped = infoError ?? lsError;
  return (
    <div className="divide-y divide-border">
      {stopped && (
        <div role="alert" data-testid="set-drive-error" className="flex items-center gap-3 py-3 text-[13px] text-error">
          <span className="min-w-0 flex-1">{stopped}</span>
          <Button
            variant="outline"
            className="h-8"
            onClick={() => {
              retry();
              void loadInfo();
            }}
          >
            重试
          </Button>
        </div>
      )}
      <Row label="ccrpc" testId="set-ccrpc">
        <code className="break-all font-mono text-[13px]">{info?.ccrpcPath ?? (infoError ? "-" : "…")}</code>
        <div className="mt-1 text-[12px] text-secondary">依次取登录 shell 环境变量 CODECK_CCRPC、~/.local/bin/ccrpc、PATH 里的 ccrpc。</div>
      </Row>
      <Row label="ccrpc doctor" testId="set-doctor">
        {info ? (
          <ul className="m-0 flex list-none flex-col gap-1 p-0 text-[13px]">
            {info.doctor.map((d) => {
              const [, name, ...rest] = d.text.split("\t");
              return (
                <li
                  key={d.text}
                  data-level={d.level}
                  className={cn("flex gap-2", d.level === "fail" && "rounded bg-error-bg px-2 py-1 font-semibold text-error", d.level === "warn" && "text-live-waiting")}
                >
                  <span className="w-10 shrink-0 font-mono text-[12px] uppercase">{d.level}</span>
                  <span className="min-w-0 break-all">{[name, rest.join(" ")].filter(Boolean).join("：")}</span>
                </li>
              );
            })}
          </ul>
        ) : (
          "…"
        )}
      </Row>
      <Row label="界面分析器" testId="set-analyzer">
        {info ? <AnalyzerForm a={info.analyzer} /> : "…"}
      </Row>
      <Row label="日志目录" testId="set-drive-logs">
        <code className="break-all font-mono text-[13px]">{info?.logRoot ?? "…"}</code>
      </Row>
    </div>
  );
}

// ccrpc's own defaults (crates/ccrpc/src/analyze.rs), shown when neither the settings nor the login shell set one.
const DEFAULT_BASE_URL = "https://api.deepseek.com/anthropic";
const DEFAULT_MODEL = "deepseek-flash";

/**
 * The analyzer ccrpc hands unrecognized dialogs to: key, endpoint and model saved here go over the login shell's
 * `CCRPC_ANALYZER_*` for the connections made from now on.
 */
function AnalyzerForm({ a }: { a: AnalyzerInfo }) {
  const save = useDrive((s) => s.saveAnalyzer);
  const [key, setKey] = useState("");
  const [baseUrl, setBaseUrl] = useState(a.baseUrl ?? "");
  const [model, setModel] = useState(a.model ?? "");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const submit = async (apiKey: string | null) => {
    setBusy(true);
    setResult(null);
    try {
      await save(apiKey, baseUrl, model);
      setKey("");
      setResult({ ok: true, text: "已保存，之后新建的驾驶连接生效" });
    } catch (e) {
      setResult({ ok: false, text: `保存失败：${e && typeof e === "object" && "message" in e ? String(e.message) : String(e)}` });
    }
    setBusy(false);
  };
  const keyNote =
    a.keySource === "settings"
      ? `已开启 · Key 保存在这里（…${a.keyHint}）`
      : a.keySource === "shell"
        ? `已开启 · Key 来自登录 shell 的 CCRPC_ANALYZER_API_KEY（…${a.keyHint}）`
        : "未开启：没有 API Key";
  const field = "grid grid-cols-[72px_1fr] items-center gap-2";
  return (
    <form
      className="flex flex-col gap-2 text-[13px]"
      onSubmit={(e) => {
        e.preventDefault();
        void submit(key.trim() ? key : null);
      }}
    >
      <div data-testid="analyzer-status" className={cn(a.enabled ? "text-text" : "text-secondary")}>
        {keyNote}
      </div>
      <label className={field}>
        <span className="text-secondary">API Key</span>
        <Input
          type="password"
          autoComplete="off"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          placeholder={a.keySource === "settings" ? "留空不改" : "填入后覆盖登录 shell 的设置"}
          data-testid="analyzer-key"
        />
      </label>
      <label className={field}>
        <span className="text-secondary">Endpoint</span>
        <Input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder={a.shellBaseUrl ?? DEFAULT_BASE_URL} data-testid="analyzer-base-url" />
      </label>
      <label className={field}>
        <span className="text-secondary">模型</span>
        <Input value={model} onChange={(e) => setModel(e.target.value)} placeholder={a.shellModel ?? DEFAULT_MODEL} data-testid="analyzer-model" />
      </label>
      <div className="flex items-center gap-2">
        <Button type="submit" className="h-8" disabled={busy} data-testid="analyzer-save">
          保存
        </Button>
        {a.keySource === "settings" && (
          <Button type="button" variant="outline" className="h-8" disabled={busy} onClick={() => void submit("")} data-testid="analyzer-clear-key">
            清除保存的 Key
          </Button>
        )}
        {result && <span className={cn("text-[12px]", result.ok ? "text-secondary" : "text-error")}>{result.text}</span>}
      </div>
      <div className="text-[12px] text-secondary">
        Anthropic Messages 协议（POST Endpoint/v1/messages），留空的项用登录 shell 的 CCRPC_ANALYZER_* 或 ccrpc 的默认值。Key 存在本机设置文件，仅本人可读。
      </div>
    </form>
  );
}
