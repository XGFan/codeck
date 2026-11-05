import { Fragment, type ReactNode } from "react";
import type { Stats } from "@/ipc/bindings";
import { cn } from "@/lib/cn";
import { formatCost, formatCount, formatTokens } from "@/lib/format";
import { inputTotals, longestStreak, perDay } from "./overview";

export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  small,
}: {
  label: string;
  value: T;
  options: { key: T; label: string }[];
  onChange: (k: T) => void;
  small?: boolean;
}) {
  return (
    <div role="group" aria-label={label} className="flex overflow-hidden rounded-md border border-border bg-ground">
      {options.map((o, i) => (
        <button
          key={o.key}
          type="button"
          aria-pressed={value === o.key}
          onClick={() => onChange(o.key)}
          className={cn(
            "px-3 text-[12px] outline-none focus-visible:ring-2 focus-visible:ring-accent",
            small ? "h-6 px-2 text-[11px]" : "h-7",
            i > 0 && "border-l border-border",
            // No bold for the active option: a wider label would shift the right-aligned group.
            value === o.key ? "bg-selection text-accent" : "text-secondary hover:bg-code",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Card({
  title,
  aside,
  className,
  children,
  testId,
}: {
  title: ReactNode;
  aside?: ReactNode;
  className?: string;
  children: ReactNode;
  testId?: string;
}) {
  return (
    <section
      data-testid={testId}
      className={cn(
        "flex min-h-0 min-w-0 flex-col gap-2 rounded-[10px] border border-border bg-ground px-3.5 py-3",
        className,
      )}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h2 className="m-0 flex-1 text-[13px] font-semibold">{title}</h2>
        {aside}
      </div>
      {children}
    </section>
  );
}

export const Empty = ({ text = "所选范围内没有数据" }: { text?: string }) => (
  <div className="flex min-h-16 flex-1 items-center justify-center text-[12px] text-secondary">{text}</div>
);

const CARD = "flex min-w-0 flex-col gap-0.5 rounded-[10px] border border-border bg-ground px-3.5 py-3";

/** Input (new, cache writes, cache reads) and output; the bar shows what the input was made of. */
function TokenCard({ stats }: { stats: Stats }) {
  const t = inputTotals(stats.daily);
  const totalIn = t.input + t.read + t.write;
  const pct = (v: number) => (totalIn ? (v / totalIn) * 100 : 0);
  const parts = [
    {
      label: "缓存读取",
      value: t.read,
      className: "bg-accent",
      hint: "已在缓存中的前缀（系统提示词、之前的对话），约按 0.1 倍输入单价计费",
    },
    {
      label: "缓存写入",
      value: t.write,
      className: "bg-accent/45",
      hint: "本次写入缓存、下一轮可按缓存读取复用的内容，按 1.25 倍（5 分钟）或 2 倍（1 小时）输入单价计费",
    },
    { label: "未缓存输入", value: t.input, className: "bg-text/20", hint: "既未读取也未写入缓存的输入，按原价计费" },
  ];
  return (
    <div data-testid="stat-card" data-card="tokens" className={cn(CARD, "col-span-2 gap-[5px]")}>
      <div className="flex items-baseline gap-2">
        <span className="text-[12px] text-secondary">Token</span>
        <span className="flex-1" />
        <span data-testid="cache-hit" className="text-[12px] text-secondary">
          缓存命中率 <span className="font-semibold text-text">{Math.round(pct(t.read))}%</span>
        </span>
      </div>
      {/* Two fixed columns: a longer number on the left never pushes the output figure around. */}
      <div className="grid grid-cols-2 gap-3">
        <div data-testid="token-input" className="flex min-w-0 items-baseline gap-1.5">
          <span className="text-[24px] leading-tight font-semibold tracking-tight">{formatTokens(totalIn)}</span>
          <span className="text-[12px] text-secondary">输入</span>
        </div>
        <div data-testid="token-output" className="flex min-w-0 items-baseline gap-1.5">
          <span className="text-[24px] leading-tight font-semibold tracking-tight">
            {formatTokens(stats.overview.outputTokens)}
          </span>
          <span className="text-[12px] text-secondary">输出</span>
        </div>
      </div>
      <div
        role="img"
        aria-label={`输入构成：${parts.map((p) => `${p.label} ${Math.round(pct(p.value))}%`).join("，")}`}
        className="flex h-1.5 overflow-hidden rounded-full bg-border"
      >
        {parts.map((p) => (
          <span key={p.label} className={p.className} style={{ width: `${pct(p.value)}%` }} />
        ))}
      </div>
      <div className="flex flex-wrap gap-x-3 text-[11px] text-secondary">
        {parts.map((p) => (
          <span key={p.label} title={p.hint} className="flex cursor-default items-center gap-1 whitespace-nowrap">
            <span className={cn("size-2 rounded-[2px]", p.className)} />
            {p.label} {formatTokens(p.value)}
          </span>
        ))}
      </div>
    </div>
  );
}

export function Overview({ stats, rangeDays }: { stats: Stats; rangeDays: number | null }) {
  const { overview: o, daily } = stats;
  const activeDays = daily.map((d) => d.day);
  const span = rangeDays ?? Math.max(o.activeDays, stats.heatDaily.length);
  const cards = [
    {
      id: "sessions",
      label: "Session",
      value: formatCount(o.sessions),
      sub: o.activeDays ? `日均 ${perDay(o.sessions / o.activeDays)} 个` : "—",
    },
    {
      id: "messages",
      label: "消息",
      value: formatCount(o.messages),
      sub: o.activeDays ? `日均 ${formatCount(Math.round(o.messages / o.activeDays))} 条` : "—",
    },
    {
      id: "cost",
      label: "Cost",
      value: formatCost(o.cost),
      sub:
        o.unpricedMessages > 0
          ? `${formatCount(o.unpricedMessages)} 条消息无价格，未计入`
          : o.activeDays
            ? `日均 ${formatCost(o.cost / o.activeDays)}`
            : "—",
    },
    {
      id: "active",
      label: "活跃天数",
      value: `${o.activeDays} / ${span}`,
      sub: activeDays.length ? `最长连续 ${longestStreak(activeDays)} 天` : "—",
    },
  ];
  return (
    <div className="grid grid-cols-2 gap-3 @2xl:grid-cols-4 @5xl:grid-cols-6">
      {cards.map((c, i) => (
        <Fragment key={c.id}>
          {/* Token sits third, after Session and messages. */}
          {i === 2 && <TokenCard stats={stats} />}
          <div data-testid="stat-card" data-card={c.id} className={CARD}>
            <span className="text-[12px] text-secondary">{c.label}</span>
            <span className="text-[24px] leading-tight font-semibold tracking-tight">{c.value}</span>
            <span className="truncate text-[11px] text-secondary">{c.sub}</span>
          </div>
        </Fragment>
      ))}
    </div>
  );
}

export function Bar({ pct, color, className }: { pct: number; color: string; className?: string }) {
  return (
    <span className={cn("block h-1.5 rounded-[3px] bg-code", className)}>
      <span className="block h-1.5 rounded-[3px]" style={{ width: `${Math.max(2, pct)}%`, background: color }} />
    </span>
  );
}
