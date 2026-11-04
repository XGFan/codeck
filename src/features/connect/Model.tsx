import { ChevronDown } from "lucide-react";
import { useState } from "react";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/cn";
import { isComposingKey } from "@/lib/hotkeys";
import type { ConnState } from "./conn";
import { errorKey, isLive, useConnect } from "./store";

const FAMILIES = ["opus", "fable", "sonnet", "haiku"] as const;

/**
 * A model as Claude Code Desktop names it: `claude-opus-4-7` → `Opus 4.7`, `claude-fable-5-1[1m]` →
 * `Fable 5.1 (1M)`; a display name (what `/model` just said it set) or an unknown id stays as it is.
 */
export function modelLabel(model: string): string {
  const m = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?(\[1m\])?$/i.exec(model.trim());
  if (!m) return model;
  const family = m[1]!.charAt(0).toUpperCase() + m[1]!.slice(1).toLowerCase();
  return `${family} ${m[2]}${m[3] ? `.${m[3]}` : ""}${m[4] ? " (1M)" : ""}`;
}

/** The family a model belongs to, for the check mark among the aliases. */
export const modelFamily = (model: string | null) => FAMILIES.find((f) => model?.toLowerCase().includes(f)) ?? null;

/** Effort levels as Desktop shows them: xhigh is 「Extra」. */
export const EFFORTS = [
  { level: "low", label: "Low" },
  { level: "medium", label: "Medium" },
  { level: "high", label: "High" },
  { level: "xhigh", label: "Extra" },
  { level: "max", label: "Max" },
] as const;

export const effortLabel = (effort: string) => EFFORTS.find((e) => e.level === effort)?.label ?? effort.charAt(0).toUpperCase() + effort.slice(1);

/** The aliases `/model` takes; 默认 follows the account's default. */
const MODELS = [
  { alias: "default", label: "默认" },
  { alias: "opus", label: "Opus" },
  { alias: "fable", label: "Fable" },
  { alias: "sonnet", label: "Sonnet" },
  { alias: "haiku", label: "Haiku" },
] as const;

/**
 * The model and effort picker at the right of the input box's foot (as in Claude Code Desktop): the model,
 * with its effort level in grey; a menu of models, 「Effort ›」 (only for a model that has levels) and
 * 「其他模型 ›」 for a name typed in. Idle only — Claude Code takes `/model` and `/effort` then; what is picked
 * also becomes the default for new Sessions, as in the terminal. Switching mid-conversation, Claude Code may
 * ask first (a choice card above the box).
 */
export function ModelPicker({ c }: { c: ConnState }) {
  const command = useConnect((s) => s.command);
  const setError = useConnect((s) => s.setError);
  const [other, setOther] = useState("");
  const [open, setOpen] = useState(false);
  const idle = isLive(c) && c.state === "idle";
  const family = modelFamily(c.model);
  const run = async (msg: Record<string, unknown>, what: string) => {
    setOpen(false);
    const r = await command(c.conn, msg);
    if (!r.success) setError(errorKey(c), `${what}失败：${r.error?.message ?? r.error?.code ?? ""}`);
  };
  const setModel = (model: string) => void run({ type: "set_model", model }, "切换模型");
  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          data-testid="connect-model"
          disabled={!idle}
          title={idle ? "切换模型与思考等级" : "空闲时可切换模型与思考等级"}
          className="ml-1 flex h-7 max-w-56 items-center gap-1.5 rounded-lg px-2 text-text/90 hover:bg-selection/60 disabled:hover:bg-transparent data-[state=open]:bg-selection/60"
        >
          <span className="truncate">{c.model ? modelLabel(c.model) : "模型"}</span>
          {c.effort && (
            <span data-testid="connect-effort" className="shrink-0 text-secondary">
              {effortLabel(c.effort)}
            </span>
          )}
          <ChevronDown size={11} className="shrink-0 opacity-60" aria-hidden />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" side="top" className="min-w-56" data-testid="model-menu">
        <DropdownMenuLabel>模型 · 也会成为新 Session 的默认</DropdownMenuLabel>
        {MODELS.map((m) => (
          <DropdownMenuCheckboxItem key={m.alias} checked={m.alias === family} onCheckedChange={() => setModel(m.alias)}>
            {m.label}
          </DropdownMenuCheckboxItem>
        ))}
        {c.effort && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuSub>
              <DropdownMenuSubTrigger data-testid="effort-trigger">
                Effort
                <span className="ml-auto text-secondary">{effortLabel(c.effort)}</span>
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className="w-64" data-testid="effort-menu">
                <div className="px-2 pt-1 pb-1.5 text-[12px] leading-snug text-secondary">思考等级越高，回答越周全，但更慢、更耗用量。</div>
                {EFFORTS.map((e) => (
                  <DropdownMenuCheckboxItem
                    key={e.level}
                    checked={e.level === c.effort}
                    onCheckedChange={() => void run({ type: "set_effort", level: e.level }, "切换思考等级")}
                  >
                    {e.label}
                  </DropdownMenuCheckboxItem>
                ))}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          </>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuSub>
          <DropdownMenuSubTrigger data-testid="other-model-trigger">其他模型</DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-64 p-2">
            <form
              className="flex items-center gap-1.5"
              onSubmit={(e) => {
                e.preventDefault();
                const name = other.trim();
                if (name) setModel(name);
              }}
            >
              <input
                value={other}
                onChange={(e) => setOther(e.target.value)}
                // Keys stay in the box: the menu would take them for typeahead and arrows.
                onKeyDown={(e) => {
                  if (e.key !== "Escape" && !isComposingKey(e.nativeEvent)) e.stopPropagation();
                }}
                placeholder="模型名，如 claude-opus-4-6"
                aria-label="模型名"
                data-testid="other-model"
                className="h-7 min-w-0 flex-1 rounded-md border border-border bg-ground px-2 text-[13px] outline-none focus:border-accent"
              />
              <button
                type="submit"
                disabled={!other.trim()}
                className={cn("h-7 shrink-0 rounded-md bg-accent px-2.5 text-[12.5px] text-white disabled:opacity-40")}
              >
                切换
              </button>
            </form>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
