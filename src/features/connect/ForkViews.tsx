// Fork's controls: 「Fork」 under a Main Line prompt (a menu of the two restores), 「Fork」 in the header's 「···」 menu
// (the whole Main Line), and 「Fork 中…」 in the header while one is under way.
import { GitFork } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/cn";
import { useSession } from "@/queries";
import { useUi } from "@/state/ui";
import { forkOffer, type ForkOffer } from "./fork";
import { pill } from "./Header";
import { useConnect } from "./store";

/** Whether the indexed Session `sessionId` can be forked now, and starting it (`at`: the prompt; null: all of it). */
function useFork(sessionId: string | null) {
  const { data: d } = useSession(sessionId);
  const ccrpcError = useConnect((s) => s.lsError ?? s.infoError);
  const forking = useConnect((s) => (sessionId ? s.forking[sessionId] : undefined));
  const fork = useConnect((s) => s.fork);
  const offer: ForkOffer = d
    ? forkOffer({ cwd: d.cwd, cwdMissing: d.cwdMissing, ccrpcError })
    : { kind: "disabled", why: "Session 还没有读进来" };
  const start = (at: string | null, restore: "conversation" | "both" | null) => {
    if (!sessionId || !d?.cwd || offer.kind !== "enabled") return;
    void fork({ sessionId, cwd: d.cwd, title: d.summary.title, at, restore });
  };
  return { offer, forking, start };
}

/**
 * 「Fork」 under a Main Line prompt (not the first), on hover: 「只恢复对话」 (the code stays as it is) or 「恢复代码与
 * 对话」. Any indexed Session, Connected or not; one Fork at a time per Session.
 */
export function ForkButton({ sessionId, nodeId }: { sessionId: string; nodeId: string }) {
  const { offer, forking, start } = useFork(sessionId);
  const running = forking?.at === nodeId;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          data-testid="fork-button"
          disabled={offer.kind === "disabled" || !!forking}
          title={offer.kind === "disabled" ? offer.why : forking ? "这个 Session 正在 Fork" : "从这条提示词之前 Fork"}
          onClick={(e) => e.stopPropagation()}
          className={cn(
            "flex h-5 items-center gap-1 rounded px-1.5 text-[11px] transition-opacity disabled:cursor-default",
            running
              ? "bg-selection text-accent"
              : "text-secondary opacity-0 group-focus-within/copy:opacity-100 group-hover/copy:opacity-100 hover:bg-selection hover:text-text focus-visible:opacity-100 disabled:hover:bg-transparent disabled:hover:text-secondary data-[state=open]:opacity-100",
          )}
        >
          <GitFork size={11} strokeWidth={1.8} aria-hidden />
          {running ? "Fork 中…" : "Fork"}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" data-testid="fork-menu" className="w-72">
        <DropdownMenuItem data-testid="fork-conversation" onSelect={() => start(nodeId, "conversation")}>
          <ForkItem label="Fork · 只恢复对话" desc="代码保持现状" />
        </DropdownMenuItem>
        <DropdownMenuItem data-testid="fork-both" onSelect={() => start(nodeId, "both")}>
          <ForkItem
            label="Fork · 恢复代码与对话"
            desc="启动目录里的文件恢复到这条提示词之前，Origin Session 也在用这些文件"
          />
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ForkItem({ label, desc }: { label: string; desc: string }) {
  return (
    <span className="flex min-w-0 items-start gap-2">
      <GitFork size={13} strokeWidth={1.5} className="mt-[3px] shrink-0" aria-hidden />
      <span className="flex min-w-0 flex-col">
        <span>{label}</span>
        <span className="text-[11.5px] text-secondary">{desc}</span>
      </span>
    </span>
  );
}

/** 「Fork」 in the header's 「···」 menu: a new Session with the whole Main Line, its input box empty. */
export function ForkMenuItem({ sessionId }: { sessionId: string }) {
  const { offer, forking, start } = useFork(sessionId);
  return (
    <DropdownMenuItem
      data-testid="fork-session"
      disabled={offer.kind === "disabled" || !!forking}
      title="复制整条 Main Line 到新 Session，在新的 clay Pane 里 Connect"
      className="data-[disabled]:text-secondary"
      onSelect={() => start(null, null)}
    >
      <GitFork size={13} strokeWidth={1.5} aria-hidden />
      {forking ? "Fork 中…" : "Fork"}
      {offer.kind === "disabled" && <span className="ml-3 text-[11.5px] text-secondary">{offer.why}</span>}
    </DropdownMenuItem>
  );
}

/**
 * 「Fork 中…」 in the Origin Session's header until the Fork is done or failed; once its Session is named, a click
 * opens it (a card there may wait on the Fork, and it has no list row until it is indexed).
 */
export function ForkPending({ sessionId }: { sessionId: string }) {
  const forking = useConnect((s) => s.forking[sessionId]);
  if (!forking) return null;
  const target = forking.sessionId;
  return (
    <button
      type="button"
      data-testid="fork-pending"
      disabled={!target}
      onClick={() => target && useUi.getState().selectSession(target)}
      className={cn(pill, "disabled:opacity-60")}
    >
      <GitFork size={13} strokeWidth={1.6} aria-hidden />
      Fork 中…
    </button>
  );
}
