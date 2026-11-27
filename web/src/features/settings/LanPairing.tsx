import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { format } from "date-fns";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { api } from "@/ipc";
import type { PairingOffer } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { errorText } from "@/lib/errors";
import { formatRelative } from "@/lib/format";
import { queryKeys } from "@/queries";

const PAIR_SUCCESS_CLOSE_MS = 1500;
const COUNTDOWN_TICK_MS = 250;

function formatCountdown(ms: number): string {
  const totalSec = Math.ceil(Math.max(0, ms) / 1000);
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

/** Modal that starts a one-time LAN pairing offer, tracks its countdown, and closes once redeemed. */
export function PairDeviceDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [offer, setOffer] = useState<PairingOffer | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pairedName, setPairedName] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  // Tracks the active unredeemed offer so any close cancels it, and paired events match only our offer.
  const activeOfferIdRef = useRef<string | null>(null);
  const aliveRef = useRef(false);
  const reqSeqRef = useRef(0);
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onOpenChangeRef = useRef(onOpenChange);
  useEffect(() => {
    onOpenChangeRef.current = onOpenChange;
  }, [onOpenChange]);

  const requestOffer = useCallback(async () => {
    const seq = ++reqSeqRef.current;
    setLoading(true);
    setError(null);
    setCopied(false);
    try {
      const next = await api.lanPairStart();
      if (!aliveRef.current || reqSeqRef.current !== seq) {
        // Closed or superseded while the RPC was in flight: drop the stale offer.
        void api.lanPairCancel(next.id);
        return;
      }
      activeOfferIdRef.current = next.id;
      setOffer(next);
      setNow(Date.now());
    } catch (e) {
      if (!aliveRef.current || reqSeqRef.current !== seq) return;
      setError(errorText(e));
    } finally {
      if (aliveRef.current && reqSeqRef.current === seq) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    aliveRef.current = true;
    activeOfferIdRef.current = null;
    setOffer(null);
    setError(null);
    setPairedName(null);
    setCopied(false);

    let unlisten: (() => void) | null = null;
    let closeTimer: ReturnType<typeof setTimeout> | null = null;

    // Subscribe to `lan-devices-event` before calling `lanPairStart` so a fast redeem is never missed.
    void (async () => {
      const off = await api.onLanDevices((e) => {
        if (!aliveRef.current) return;
        if (e.pairedOffer && e.pairedOffer === activeOfferIdRef.current) {
          const name = e.devices.find((d) => d.id === e.pairedDevice)?.name ?? "设备";
          activeOfferIdRef.current = null;
          setPairedName(name);
          closeTimer = setTimeout(() => {
            if (aliveRef.current) onOpenChangeRef.current(false);
          }, PAIR_SUCCESS_CLOSE_MS);
        }
      });
      if (!aliveRef.current) {
        off();
        return;
      }
      unlisten = off;
      await requestOffer();
    })();

    return () => {
      aliveRef.current = false;
      if (closeTimer) clearTimeout(closeTimer);
      if (copyTimerRef.current) {
        clearTimeout(copyTimerRef.current);
        copyTimerRef.current = null;
      }
      unlisten?.();
      const id = activeOfferIdRef.current;
      activeOfferIdRef.current = null;
      if (id) void api.lanPairCancel(id);
    };
  }, [open, requestOffer]);

  const remainingMs = offer ? Math.max(0, offer.expiresAt - now) : 0;
  const expired = offer != null && pairedName == null && remainingMs <= 0;

  useEffect(() => {
    if (!open || !offer || expired || pairedName != null) return;
    const id = setInterval(() => setNow(Date.now()), COUNTDOWN_TICK_MS);
    return () => clearInterval(id);
  }, [open, offer, expired, pairedName]);

  const copyLink = async () => {
    if (!offer) return;
    await api.copyText(offer.url);
    setCopied(true);
    if (copyTimerRef.current) clearTimeout(copyTimerRef.current);
    copyTimerRef.current = setTimeout(() => setCopied(false), 1500);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-testid="lan-pair-dialog" aria-describedby={undefined} className="w-[420px] p-5">
        <DialogTitle className="mb-3 text-[15px] font-semibold">配对新设备</DialogTitle>
        <DialogDescription className="sr-only">扫描二维码或使用配对链接将 iPhone 与这台 Mac 配对</DialogDescription>

        {pairedName != null ? (
          <div data-testid="lan-pair-success" className="py-8 text-center text-[15px] font-medium text-text">
            已配对 {pairedName}
          </div>
        ) : (
          <div className="flex flex-col items-center gap-3">
            {offer ? (
              <>
                <img
                  src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(offer.svg)}`}
                  alt="配对二维码"
                  data-testid="lan-pair-qr"
                  className={cn(
                    "h-[200px] w-[200px] rounded-lg border border-border bg-white p-2 transition-opacity",
                    expired && "opacity-30",
                  )}
                />
                <div className="text-[13px] text-secondary">用 iPhone 上的 CoDeck 扫码</div>
                <div className="flex w-full items-center gap-2">
                  <code
                    data-testid="lan-pair-url"
                    className="min-w-0 flex-1 select-all break-all rounded-md border border-border bg-sidebar px-2.5 py-1.5 font-mono text-[12px] text-text"
                  >
                    {offer.url}
                  </code>
                  <Button
                    type="button"
                    variant="outline"
                    className="h-8 shrink-0"
                    data-testid="lan-pair-copy"
                    onClick={() => void copyLink()}
                  >
                    {copied ? "已复制" : "复制"}
                  </Button>
                </div>
                {expired ? (
                  <div className="flex items-center gap-3">
                    <span data-testid="lan-pair-expired" className="text-[13px] text-error">
                      配对码已过期
                    </span>
                    <Button
                      type="button"
                      variant="outline"
                      className="h-8"
                      disabled={loading}
                      data-testid="lan-pair-regenerate"
                      onClick={() => void requestOffer()}
                    >
                      重新生成
                    </Button>
                  </div>
                ) : (
                  <div data-testid="lan-pair-countdown" className="text-[13px] tabular-nums text-secondary">
                    {formatCountdown(remainingMs)} 后失效
                  </div>
                )}
              </>
            ) : loading ? (
              <div className="py-10 text-[13px] text-secondary">正在生成配对码…</div>
            ) : null}

            {error && (
              <div className="flex flex-col items-center gap-2">
                <div role="alert" data-testid="lan-pair-error" className="text-center text-[13px] text-error">
                  {error}
                </div>
                {!offer && (
                  <Button
                    type="button"
                    variant="outline"
                    className="h-8"
                    disabled={loading}
                    data-testid="lan-pair-regenerate"
                    onClick={() => void requestOffer()}
                  >
                    重新生成
                  </Button>
                )}
              </div>
            )}
          </div>
        )}

        <div className="mt-4 flex justify-end">
          <Button
            type="button"
            variant="outline"
            className="h-8"
            data-testid="lan-pair-close"
            onClick={() => onOpenChange(false)}
          >
            关闭
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** List of paired LAN devices; shown whenever at least one device is paired, even while LAN access is off. */
export function PairedDevicesList() {
  const qc = useQueryClient();
  const [confirmId, setConfirmId] = useState<string | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);

  const { data: devices = [] } = useQuery({
    queryKey: queryKeys.lanDevices,
    queryFn: () => api.lanDevices(),
    refetchOnMount: "always",
  });

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | null = null;
    void api
      .onLanDevices((e) => {
        if (!alive) return;
        void qc.cancelQueries({ queryKey: queryKeys.lanDevices });
        qc.setQueryData(queryKeys.lanDevices, e.devices);
      })
      .then((off) => {
        if (alive) unlisten = off;
        else off();
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [qc]);

  const remove = useMutation({
    mutationFn: (id: string) => api.lanRemoveDevice(id),
    onSuccess: () => {
      setConfirmId(null);
      setRemoveError(null);
    },
    onError: (e) => setRemoveError(errorText(e)),
  });

  if (devices.length === 0) return null;

  return (
    <div data-testid="lan-devices" className="mt-2 flex flex-col gap-1.5 border-t border-border pt-2.5">
      <div className="text-[12px] font-medium text-secondary">已配对的设备</div>
      <div className="divide-y divide-border rounded-lg border border-border">
        {devices.map((d) => {
          const confirming = confirmId === d.id;
          return (
            <div
              key={d.id}
              data-testid="lan-device-row"
              data-device-id={d.id}
              className="flex items-center justify-between gap-3 px-3 py-2"
            >
              <div className="min-w-0">
                <div className="truncate text-[13px] font-medium text-text">{d.name}</div>
                <div className="text-[12px] text-secondary">
                  配对于 {format(d.pairedAt, "yyyy-MM-dd HH:mm")} ·{" "}
                  {d.lastSeenAt != null ? `最近连接 ${formatRelative(d.lastSeenAt)}` : "从未连接"}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-1.5">
                {confirming ? (
                  <>
                    <Button
                      type="button"
                      variant="outline"
                      className="h-7 border-error/40 px-2.5 text-[12px] text-error hover:bg-error/10"
                      disabled={remove.isPending}
                      data-testid="lan-device-confirm-remove"
                      onClick={() => remove.mutate(d.id)}
                    >
                      确认移除
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      className="h-7 px-2.5 text-[12px]"
                      disabled={remove.isPending}
                      data-testid="lan-device-cancel-remove"
                      onClick={() => setConfirmId(null)}
                    >
                      取消
                    </Button>
                  </>
                ) : (
                  <Button
                    type="button"
                    variant="outline"
                    className="h-7 px-2.5 text-[12px]"
                    data-testid="lan-device-remove"
                    onClick={() => {
                      setRemoveError(null);
                      setConfirmId(d.id);
                    }}
                  >
                    移除
                  </Button>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {removeError && (
        <div role="alert" data-testid="lan-device-error" className="text-[12px] text-error">
          {removeError}
        </div>
      )}
    </div>
  );
}
