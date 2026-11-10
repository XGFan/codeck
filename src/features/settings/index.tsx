import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Settings } from "lucide-react";
import { Tabs } from "radix-ui";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { DiagnosticsContent } from "@/features/diagnostics";
import { CcrpcSettings } from "@/features/connect";
import { formatCount } from "@/lib/format";
import { useHotkey } from "@/lib/hotkeys";
import { api } from "@/ipc";
import type { DataRootSource } from "@/ipc/bindings";
import { queryKeys, useAppInfo, useIndexStatus } from "@/queries";
import { CONV_WIDTH, useUi } from "@/state/ui";
import { errorText } from "@/lib/errors";
import { cn } from "@/lib/cn";
import { setThemePref, type ThemePref, useThemePref } from "@/lib/theme";

const SOURCE_LABEL: Record<DataRootSource, string> = {
  settings: "设置",
  env: "环境变量 CLAUDE_CONFIG_DIR",
  default: "默认",
};
const TABS = [
  { id: "general", label: "通用" },
  { id: "data", label: "数据" },
  { id: "ccrpc", label: "ccrpc" },
  { id: "compat", label: "格式兼容性" },
] as const;

const WEEKDAY_NAMES = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
const pad = (n: number) => String(n).padStart(2, "0");

function Row({ label, children, testId }: { label: string; children: React.ReactNode; testId?: string }) {
  return (
    <div data-testid={testId} className="grid grid-cols-[140px_1fr] items-baseline gap-4 py-3 text-[14px]">
      <div className="text-secondary">{label}</div>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

/** When the stats page's 本周期 starts over (local weekday and time). */
function WeeklyResetField() {
  const reset = useUi((s) => s.weeklyReset);
  const setReset = useUi((s) => s.setWeeklyReset);
  const field =
    "h-8 rounded-md border border-border bg-ground px-2 text-[14px] text-text outline-none focus-visible:ring-2 focus-visible:ring-accent";
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex flex-wrap items-center gap-2">
        <span>每周</span>
        <select
          aria-label="重置日"
          className={field}
          value={reset.weekday}
          onChange={(e) => setReset({ ...reset, weekday: Number(e.target.value) })}
        >
          {WEEKDAY_NAMES.map((n, i) => (
            <option key={n} value={i}>
              {n}
            </option>
          ))}
        </select>
        <input
          type="time"
          aria-label="重置时间"
          className={field}
          value={`${pad(Math.floor(reset.minutes / 60))}:${pad(reset.minutes % 60)}`}
          onChange={(e) => {
            const [h, m] = e.target.value.split(":").map(Number);
            if (Number.isFinite(h) && Number.isFinite(m)) setReset({ ...reset, minutes: h * 60 + m });
          }}
        />
        <span>重置</span>
      </div>
      <div className="text-[12px] text-secondary">统计页的「本周期」从最近一次重置算起，按本机时区。</div>
    </div>
  );
}

/** The conversation column's width (`CONV_WIDTH`), centered in the pane; narrower panes narrow it too. */
function ConvWidthField() {
  const width = useUi((s) => s.convWidth);
  const setWidth = useUi((s) => s.setConvWidth);
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-3">
        <input
          type="range"
          aria-label="对话宽度"
          min={CONV_WIDTH.min}
          max={CONV_WIDTH.max}
          step={20}
          value={width}
          onChange={(e) => setWidth(Number(e.target.value))}
          className="w-56 accent-accent"
        />
        <span data-testid="conv-width-value" className="w-16 text-[13px] tabular-nums">
          {width} px
        </span>
        <Button variant="outline" disabled={width === CONV_WIDTH.default} onClick={() => setWidth(CONV_WIDTH.default)}>
          恢复默认
        </Button>
      </div>
      <div className="text-[12px] text-secondary">
        对话记录与输入区的宽度，居中显示；窗口窄时随之变窄。默认 720 px。
      </div>
    </div>
  );
}

/** A few choices side by side, the chosen one filled (as 「新开的 Pane」 on the ccrpc tab). */
function Segmented<T extends string | boolean>({
  label,
  value,
  options,
  onChange,
  testId,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  testId: string;
}) {
  return (
    <div role="radiogroup" aria-label={label} className="flex w-fit gap-1 rounded-lg border border-border p-0.5">
      {options.map((o) => (
        <button
          key={String(o.value)}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          data-testid={`${testId}-${String(o.value)}`}
          onClick={() => onChange(o.value)}
          className={cn(
            "h-7 rounded-md px-3 text-[13px]",
            value === o.value ? "bg-selection font-semibold text-text" : "text-secondary hover:bg-selection/60",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

const THEMES: { value: ThemePref; label: string }[] = [
  { value: "auto", label: "自动" },
  { value: "light", label: "浅色" },
  { value: "dark", label: "暗色" },
];

/** 「局域网访问」: this Mac as a Host on the LAN. What it listens on follows the network: read again while shown. */
function LanAccessField() {
  const qc = useQueryClient();
  const { data: status } = useQuery({
    queryKey: queryKeys.lanStatus,
    queryFn: () => api.lanStatus(),
    refetchInterval: 5000,
  });
  const toggle = useMutation({
    mutationFn: (on: boolean) => api.lanSetEnabled(on),
    onSuccess: async (next) => {
      // A refresh under way read the state before the switch.
      await qc.cancelQueries({ queryKey: queryKeys.lanStatus });
      qc.setQueryData(queryKeys.lanStatus, next);
    },
  });
  const enabled = toggle.isPending ? toggle.variables : (status?.enabled ?? false);
  const note = "text-[12px] text-secondary";
  return (
    <div className="flex flex-col gap-1.5">
      <Segmented
        label="局域网访问"
        value={enabled}
        options={[
          { value: true, label: "开启" },
          { value: false, label: "关闭" },
        ]}
        onChange={(on) => {
          if (!toggle.isPending && on !== enabled) toggle.mutate(on);
        }}
        testId="lan-access"
      />
      <div className={note}>开启后，同一网络的设备无需验证即可连接这台 Mac 并操作 Session；关闭窗口不退出应用。</div>
      {status?.enabled && (status.addresses.length > 0 || !status.error) && (
        <div data-testid="lan-addresses" className={note}>
          {status.addresses.length > 0 ? `地址：${status.addresses.join("、")}` : "没有可用的局域网地址。"}
        </div>
      )}
      {status?.error && (
        <div data-testid="lan-error" className="text-[12px] text-error">
          {status.error}
        </div>
      )}
      {toggle.error && (
        <div data-testid="lan-toggle-error" className="text-[12px] text-error">
          {errorText(toggle.error)}
        </div>
      )}
    </div>
  );
}

function GeneralTab() {
  const { data: info } = useAppInfo();
  const theme = useThemePref();
  const autoConnect = useUi((s) => s.autoConnect);
  const setAutoConnect = useUi((s) => s.setAutoConnect);
  return (
    <div className="divide-y divide-border">
      <Row label="界面主题" testId="set-theme">
        <div className="flex flex-col gap-1.5">
          <Segmented label="界面主题" value={theme} options={THEMES} onChange={setThemePref} testId="theme" />
          <div className="text-[12px] text-secondary">「自动」跟随系统的外观设置。</div>
        </div>
      </Row>
      <Row label="自动 Connect" testId="set-auto-connect">
        <div className="flex flex-col gap-1.5">
          <Segmented
            label="自动 Connect"
            value={autoConnect}
            options={[
              { value: true, label: "开启" },
              { value: false, label: "关闭" },
            ]}
            onChange={setAutoConnect}
            testId="auto-connect"
          />
          <div className="text-[12px] text-secondary">
            打开可以 Connect 的 Session 时自动 Connect；Disconnect 后要再次打开才会再连。
          </div>
        </div>
      </Row>
      <Row label="局域网访问" testId="set-lan-access">
        <LanAccessField />
      </Row>
      <Row label="对话宽度" testId="set-conv-width">
        <ConvWidthField />
      </Row>
      <Row label="统计周期" testId="set-cycle">
        <WeeklyResetField />
      </Row>
      <Row label="应用版本" testId="set-version">
        {info?.version ?? "…"}
      </Row>
      <Row label="索引 Schema 版本" testId="set-schema">
        {info?.schemaVersion ?? "…"}
      </Row>
    </div>
  );
}

function DataTab() {
  const qc = useQueryClient();
  const { data: info } = useAppInfo();
  const { data: status } = useIndexStatus();
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (info) setDraft(info.dataRoot);
  }, [info]);

  const setRoot = useMutation({
    mutationFn: (path: string | null) => api.setDataRoot(path),
    onSuccess: (next) => {
      setError(null);
      // Everything cached came from the previous root: drop the selection and reset every query
      // (active ones refetch) except the app info set here and the pushed index status.
      useUi.getState().selectSession(null);
      void qc.resetQueries({ predicate: (q) => q.queryKey[0] !== "appInfo" && q.queryKey[0] !== "indexStatus" });
      qc.setQueryData(queryKeys.appInfo, next);
    },
    onError: (e) => setError(errorText(e)),
  });
  const rebuild = useMutation({
    mutationFn: () => api.rebuildIndex(),
    onSuccess: () => setError(null),
    onError: (e) => setError(errorText(e)),
  });

  if (!info) return null;
  const busy = status?.phase === "scanning" || status?.phase === "indexingText" || rebuild.isPending;
  const trimmed = draft.trim();
  const canApply = trimmed !== "" && trimmed !== info.dataRoot && !setRoot.isPending;
  const statusText = !status
    ? "…"
    : status.phase === "error"
      ? `索引失败${status.error ? ` · ${status.error}` : ""}`
      : status.phase === "scanning"
        ? `正在扫描 ${formatCount(status.filesDone)} / ${formatCount(status.filesTotal)} 个文件`
        : status.phase === "indexingText"
          ? "正在建立搜索索引"
          : `已索引 ${formatCount(status.sessionsTotal)} 个 Session${status.textReady ? "" : " · 搜索索引未完成"}`;

  return (
    <div className="divide-y divide-border">
      <Row label="数据目录" testId="set-root">
        <div className="flex flex-col gap-2">
          <div>
            <code data-testid="set-root-path" className="break-all font-mono text-[13px]">
              {info.dataRoot}
            </code>
            <span data-testid="set-root-source" className="ml-2 text-[12px] text-secondary">
              来源：{SOURCE_LABEL[info.dataRootSource]}
            </span>
          </div>
          {!info.dataRootExists && <div className="text-[12px] text-error">该目录不存在</div>}
          <form
            className="flex items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (canApply) setRoot.mutate(trimmed);
            }}
          >
            <Input
              aria-label="数据目录路径"
              data-testid="set-root-input"
              className="h-8 min-w-0 flex-1 font-mono text-[13px]"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              placeholder="/Users/…/.claude"
              spellCheck={false}
            />
            <Button type="submit" variant="outline" className="h-8" disabled={!canApply} data-testid="set-root-apply">
              应用
            </Button>
            <Button
              type="button"
              variant="outline"
              className="h-8"
              disabled={info.dataRootSource !== "settings" || setRoot.isPending}
              onClick={() => setRoot.mutate(null)}
              data-testid="set-root-reset"
            >
              恢复默认
            </Button>
          </form>
        </div>
      </Row>
      <Row label="缓存目录" testId="set-cache">
        <code className="break-all font-mono text-[13px]">{info.cacheDir}</code>
      </Row>
      <Row label="索引" testId="set-index">
        <div className="flex items-center gap-3">
          <span data-testid="set-index-status" className={status?.phase === "error" ? "text-error" : ""}>
            {statusText}
          </span>
          <Button
            variant="outline"
            className="h-8"
            disabled={busy}
            onClick={() => rebuild.mutate()}
            data-testid="set-rebuild"
          >
            重建索引
          </Button>
        </div>
      </Row>
      {error && (
        <div role="alert" data-testid="set-error" className="py-2 text-[12px] text-error">
          {error}
        </div>
      )}
    </div>
  );
}

export function SettingsDialog() {
  const open = useUi((s) => s.settingsOpen);
  const setOpen = useUi((s) => s.setSettingsOpen);
  const [tab, setTab] = useState<(typeof TABS)[number]["id"]>("general");

  useHotkey("Meta+,", () => setOpen(!useUi.getState().settingsOpen), { enableInInputs: true });

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent
        data-testid="settings"
        aria-describedby={undefined}
        className="flex max-h-[85vh] w-[920px] max-w-[94vw] flex-col overflow-hidden rounded-xl p-0"
      >
        <DialogTitle className="m-0 border-b border-border bg-sidebar py-3 text-center text-[15px] font-semibold">
          设置
        </DialogTitle>
        <DialogDescription className="sr-only">应用设置</DialogDescription>
        <Tabs.Root value={tab} onValueChange={(v) => setTab(v as typeof tab)} className="flex min-h-0 flex-1 flex-col">
          <Tabs.List className="flex justify-center gap-1 border-b border-border bg-sidebar p-2">
            {TABS.map((t) => (
              <Tabs.Trigger
                key={t.id}
                value={t.id}
                className="h-9 min-w-[84px] rounded-lg px-4 text-[14px] text-text/80 data-[state=active]:bg-selection data-[state=active]:font-semibold data-[state=active]:text-text focus-visible:outline-2 focus-visible:outline-accent"
              >
                {t.label}
              </Tabs.Trigger>
            ))}
          </Tabs.List>
          <div className="min-h-[320px] flex-1 overflow-y-auto px-6 py-5">
            <Tabs.Content value="general">
              <GeneralTab />
            </Tabs.Content>
            <Tabs.Content value="data">
              <DataTab />
            </Tabs.Content>
            <Tabs.Content value="ccrpc">
              <CcrpcSettings />
            </Tabs.Content>
            <Tabs.Content value="compat">
              <DiagnosticsContent />
            </Tabs.Content>
          </div>
        </Tabs.Root>
      </DialogContent>
    </Dialog>
  );
}

/** Gear button that opens the settings dialog; placed by the app shell. */
export function SettingsButton({ className }: { className?: string }) {
  const setOpen = useUi((s) => s.setSettingsOpen);
  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label="设置"
      title="设置 (⌘,)"
      className={className}
      onClick={() => setOpen(true)}
      data-testid="settings-button"
    >
      <Settings size={15} />
    </Button>
  );
}
