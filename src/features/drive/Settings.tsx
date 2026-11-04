import { useEffect } from "react";
import { Button } from "@/components/ui/button";
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
        {info ? (info.analyzer ? "已开启（CCRPC_ANALYZER_API_KEY 已设置）" : "未开启（未设置 CCRPC_ANALYZER_API_KEY）") : "…"}
      </Row>
      <Row label="日志目录" testId="set-drive-logs">
        <code className="break-all font-mono text-[13px]">{info?.logRoot ?? "…"}</code>
      </Row>
    </div>
  );
}
