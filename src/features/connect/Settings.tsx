import { X } from "lucide-react";
import { format } from "date-fns";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api } from "@/ipc";
import type { AnalyzerInfo, AnalyzerRecords, LaunchRule } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { useConnect } from "./store";
import { errorText } from "@/lib/errors";

function Row({ label, children, testId }: { label: string; children: React.ReactNode; testId?: string }) {
  return (
    <div data-testid={testId} className="grid grid-cols-[140px_1fr] items-baseline gap-4 py-3 text-[14px]">
      <div className="text-secondary">{label}</div>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

/** Settings › ccrpc: ccrpc's path, `ccrpc doctor`, the analyzer and its records, launch rules, the log directory. Asks for `ccrpcInfo` when shown. */
export function CcrpcSettings() {
  const info = useConnect((s) => s.info);
  const infoError = useConnect((s) => s.infoError);
  const lsError = useConnect((s) => s.lsError);
  const loadInfo = useConnect((s) => s.loadInfo);
  const retry = useConnect((s) => s.retry);

  useEffect(() => {
    void loadInfo();
  }, [loadInfo]);

  const stopped = infoError ?? lsError;
  return (
    <div className="divide-y divide-border">
      {stopped && (
        <div role="alert" data-testid="set-ccrpc-error" className="flex items-center gap-3 py-3 text-[13px] text-error">
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
        <div className="mt-1 text-[12px] text-secondary">
          依次取登录 shell 环境变量 CODECK_CCRPC、~/.local/bin/ccrpc、PATH 里的 ccrpc。
        </div>
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
                  className={cn(
                    "flex gap-2",
                    d.level === "fail" && "rounded bg-error-bg px-2 py-1 font-semibold text-error",
                    d.level === "warn" && "text-live-waiting",
                  )}
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
      <Row label="分析记录" testId="set-analyzer-records">
        <AnalyzerRecordsList />
      </Row>
      <Row label="新开的 Pane" testId="set-hide-new-panes">
        {info ? <HideNewPanes saved={info.hideNewPanes} /> : "…"}
      </Row>
      <Row label="启动参数" testId="set-launch-rules">
        {info ? <LaunchRulesForm saved={info.launchRules} /> : "…"}
      </Row>
      <Row label="日志目录" testId="set-ccrpc-logs">
        <code className="break-all font-mono text-[13px]">{info?.logRoot ?? "…"}</code>
      </Row>
    </div>
  );
}

/** Records shown; the rest are in the directory. */
const RECORDS_SHOWN = 10;

/** ccrpc's analysis records, newest first: when, learned or failed, the dialog's title or why it failed; the files hold the rest. */
function AnalyzerRecordsList() {
  const [data, setData] = useState<AnalyzerRecords | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  useEffect(() => {
    let gone = false;
    api.ccrpcAnalyzerRecords().then(
      (d) => !gone && setData(d),
      (e) => !gone && setError(errorText(e)),
    );
    return () => {
      gone = true;
    };
  }, []);
  const open = async () => {
    setOpenError(null);
    try {
      await api.ccrpcOpenAnalyzerRecords();
    } catch (e) {
      setOpenError(errorText(e));
    }
  };
  if (error) return <div className="text-[13px] text-error">{error}</div>;
  if (!data) return "…";
  return (
    <div className="flex flex-col gap-2 text-[13px]">
      {data.records.length === 0 ? (
        <div className="text-secondary">还没有分析记录</div>
      ) : (
        <ul className="m-0 flex list-none flex-col gap-1 p-0">
          {data.records.slice(0, RECORDS_SHOWN).map((r) => (
            <li key={r.file} data-testid="analyzer-record" className="flex gap-2">
              <span className="w-[92px] shrink-0 tabular-nums text-secondary">
                {format(new Date(r.at), "M月d日 HH:mm")}
              </span>
              <span className={cn("w-8 shrink-0", r.learned ? "text-text" : "text-error")}>
                {r.learned ? "学会" : "失败"}
              </span>
              <span className="min-w-0 break-words">
                {[r.title, r.learned ? null : r.error].filter(Boolean).join(" · ")}
              </span>
            </li>
          ))}
        </ul>
      )}
      <div className="flex items-center gap-2">
        <Button
          type="button"
          variant="outline"
          className="h-8"
          onClick={() => void open()}
          data-testid="analyzer-records-open"
        >
          打开记录目录
        </Button>
        <code className="min-w-0 break-all font-mono text-[12px] text-secondary">{data.dir}</code>
      </div>
      {openError && <div className="text-[12px] text-error">{openError}</div>}
      <div className="text-[12px] text-secondary">
        ccrpc 每次分析留一个文件，含当时的屏幕、送出的工具输入与模型回复，只留最近 200 条
        {data.records.length > RECORDS_SHOWN ? `；这里列出最近 ${RECORDS_SHOWN} 条，共 ${data.records.length} 条` : ""}
        。
      </div>
    </div>
  );
}

// ccrpc's own defaults (crates/ccrpc/src/analyze.rs), shown when neither the settings nor the login shell set one.
const DEFAULT_BASE_URL = "https://api.deepseek.com/anthropic";
const DEFAULT_MODEL = "deepseek-flash";

/**
 * The analyzer ccrpc hands unrecognized dialogs to: key, endpoint and model saved here go over the login shell's
 * `CCRPC_ANALYZER_*` for the connections made from now on. 「测试」 tries the values in the form without saving them.
 */
function AnalyzerForm({ a }: { a: AnalyzerInfo }) {
  const save = useConnect((s) => s.saveAnalyzer);
  const [key, setKey] = useState("");
  const [baseUrl, setBaseUrl] = useState(a.baseUrl ?? "");
  const [model, setModel] = useState(a.model ?? "");
  const [busy, setBusy] = useState<"save" | "test" | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const submit = async (apiKey: string | null) => {
    setBusy("save");
    setResult(null);
    try {
      await save(apiKey, baseUrl, model);
      setKey("");
      setResult({ ok: true, text: "已保存，之后新建的连接生效" });
    } catch (e) {
      setResult({
        ok: false,
        text: `保存失败：${errorText(e)}`,
      });
    }
    setBusy(null);
  };
  const test = async () => {
    setBusy("test");
    setResult(null);
    try {
      const t = await api.ccrpcTestAnalyzer(key.trim() ? key : null, baseUrl, model);
      setResult({ ok: true, text: `可用：${t.model} 用时 ${(t.elapsedMs / 1000).toFixed(1)} 秒` });
    } catch (e) {
      setResult({ ok: false, text: `不可用：${errorText(e)}` });
    }
    setBusy(null);
  };
  // A result describes the values it was made with: editing a field clears it.
  const edit = (set: (v: string) => void, v: string) => {
    set(v);
    setResult(null);
  };
  // No key in the form, the settings or the login shell: nothing to test with.
  const noKey = !a.enabled && !key.trim();
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
          onChange={(e) => edit(setKey, e.target.value)}
          disabled={busy !== null}
          placeholder={a.keySource === "settings" ? "留空不改" : "填入后覆盖登录 shell 的设置"}
          data-testid="analyzer-key"
        />
      </label>
      <label className={field}>
        <span className="text-secondary">Endpoint</span>
        <Input
          value={baseUrl}
          onChange={(e) => edit(setBaseUrl, e.target.value)}
          disabled={busy !== null}
          placeholder={a.shellBaseUrl ?? DEFAULT_BASE_URL}
          data-testid="analyzer-base-url"
        />
      </label>
      <label className={field}>
        <span className="text-secondary">模型</span>
        <Input
          value={model}
          onChange={(e) => edit(setModel, e.target.value)}
          disabled={busy !== null}
          placeholder={a.shellModel ?? DEFAULT_MODEL}
          data-testid="analyzer-model"
        />
      </label>
      <div className="flex items-center gap-2">
        <Button type="submit" className="h-8" disabled={busy !== null} data-testid="analyzer-save">
          保存
        </Button>
        <Button
          type="button"
          variant="outline"
          className="h-8"
          disabled={busy !== null || noKey}
          title={noKey ? "没有 API Key" : undefined}
          onClick={() => void test()}
          data-testid="analyzer-test"
        >
          {busy === "test" ? "正在测试…" : "测试"}
        </Button>
        {a.keySource === "settings" && (
          <Button
            type="button"
            variant="outline"
            className="h-8"
            disabled={busy !== null}
            onClick={() => void submit("")}
            data-testid="analyzer-clear-key"
          >
            清除保存的 Key
          </Button>
        )}
        {result && (
          <span className={cn("text-[12px]", result.ok ? "text-secondary" : "text-error")}>{result.text}</span>
        )}
      </div>
      <div className="text-[12px] text-secondary">
        Anthropic Messages 协议（POST Endpoint/v1/messages），留空的项用登录 shell 的 CCRPC_ANALYZER_* 或 ccrpc
        的默认值。Key 存在本机设置文件，仅本人可读。「测试」用框里的值（Key
        留空用已有的）让分析器识别一个内置对话框，不保存。
      </div>
    </form>
  );
}

/** 「新开的 Pane」: show or hide the Panes of Sessions CoDeck starts (new and Resume) on the ToH Mac Client; saved at once. */
function HideNewPanes({ saved }: { saved: boolean }) {
  const save = useConnect((s) => s.saveHideNewPanes);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const choose = async (hide: boolean) => {
    if (hide === saved || busy) return;
    setBusy(true);
    setError(null);
    try {
      await save(hide);
    } catch (e) {
      setError(`保存失败：${errorText(e)}`);
    }
    setBusy(false);
  };
  const opt = (hide: boolean, label: string) => (
    <button
      type="button"
      role="radio"
      aria-checked={saved === hide}
      disabled={busy}
      data-testid={hide ? "hide-new-panes-hide" : "hide-new-panes-show"}
      onClick={() => void choose(hide)}
      className={cn(
        "h-7 rounded-md px-3 text-[13px]",
        saved === hide ? "bg-selection font-semibold text-text" : "text-secondary hover:bg-selection/60",
      )}
    >
      {label}
    </button>
  );
  return (
    <div className="flex flex-col gap-2 text-[13px]">
      <div
        role="radiogroup"
        aria-label="新开的 Pane"
        className="flex w-fit gap-1 rounded-lg border border-border p-0.5"
      >
        {opt(false, "显示")}
        {opt(true, "隐藏")}
      </div>
      {error && <span className="text-[12px] text-error">{error}</span>}
      <div className="text-[12px] text-secondary">
        新建与 Resume 的 Session 在 clay 里新开一个 Pane。选「隐藏」时它收进 ToH Mac 的隐藏窗口，iOS 与 web
        照常显示；之后可在标题栏切换。
      </div>
    </div>
  );
}

/**
 * Extra claude arguments by directory for 「新建 Session」 (like a shell wrapper adding `--add-dir` by working
 * directory): the deepest directory covering the Session's wins.
 */
function LaunchRulesForm({ saved }: { saved: LaunchRule[] }) {
  const save = useConnect((s) => s.saveLaunchRules);
  const [rules, setRules] = useState(saved);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const edit = (i: number, args: string) => setRules((rs) => rs.map((r, j) => (j === i ? { ...r, args } : r)));
  const add = async () => {
    const dir = await api.pickDir();
    if (dir && !rules.some((r) => r.dir === dir)) setRules((rs) => [...rs, { dir, args: "" }]);
  };
  const submit = async () => {
    setBusy(true);
    setResult(null);
    try {
      await save(rules);
      setResult({ ok: true, text: "已保存，之后新建的 Session 生效" });
    } catch (e) {
      setResult({
        ok: false,
        text: `保存失败：${errorText(e)}`,
      });
    }
    setBusy(false);
  };
  useEffect(() => setRules(saved), [saved]);
  return (
    <div className="flex flex-col gap-2 text-[13px]">
      {rules.map((r, i) => (
        <div
          key={r.dir}
          data-testid="launch-rule"
          className="grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)_auto] items-center gap-2"
        >
          <code className="truncate font-mono text-[12px]" title={r.dir}>
            {r.dir}
          </code>
          <Input
            value={r.args}
            onChange={(e) => edit(i, e.target.value)}
            placeholder="--add-dir=/path/to/dir"
            aria-label={`${r.dir} 的启动参数`}
            data-testid="launch-rule-args"
          />
          <Button
            type="button"
            variant="ghost"
            className="size-8 p-0"
            title="删除"
            aria-label={`删除 ${r.dir}`}
            onClick={() => setRules((rs) => rs.filter((_, j) => j !== i))}
            data-testid="launch-rule-remove"
          >
            <X size={14} strokeWidth={1.6} aria-hidden />
          </Button>
        </div>
      ))}
      <div className="flex items-center gap-2">
        <Button
          type="button"
          variant="outline"
          className="h-8"
          disabled={busy}
          onClick={() => void add()}
          data-testid="launch-rule-add"
        >
          添加目录…
        </Button>
        <Button
          type="button"
          className="h-8"
          disabled={busy}
          onClick={() => void submit()}
          data-testid="launch-rules-save"
        >
          保存
        </Button>
        {result && (
          <span className={cn("text-[12px]", result.ok ? "text-secondary" : "text-error")}>{result.text}</span>
        )}
      </div>
      <div className="text-[12px] text-secondary">
        在目录或其子目录里新建 Session 时，把参数追加给 claude；多个目录都包含它时用最深的那个。参数按 shell 规则拆分，~
        与 $VAR 按登录 shell 的环境展开（单引号里的不展开）；参数为空的目录保存时去掉。
      </div>
    </div>
  );
}
