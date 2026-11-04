/**
 * What a `<task-notification>` is about. Claude Code sends one when any background task ends — an
 * agent, a shell command, a Monitor, a Workflow — and says which in its English summary:
 * `Background command "…" completed (exit code 0)`, `Agent "…" finished`, `Monitor "…" stream ended`…
 */
export interface Notice {
  /** 后台 agent / 后台命令 / Monitor / Workflow / 后台任务. */
  kind: string;
  /** The task's description (a command's own text when it had none). */
  name: string | null;
  /** What the status does not say: a nonzero exit code, why it failed, or the whole summary when unrecognized. */
  detail: string | null;
}

const KINDS: Record<string, string> = {
  "Background command": "后台命令",
  Agent: "后台 agent",
  Monitor: "Monitor",
  "Dynamic workflow": "Workflow",
  Workflow: "Workflow",
};

const SHAPE = /^(Background command|Agent|Monitor|Dynamic workflow|Workflow) "([\s\S]*)"\s*([\s\S]*)$/;

export function describeNotice(summary: string | null, agent: string | null): Notice {
  const m = summary ? SHAPE.exec(summary.trim()) : null;
  if (!m) return agent ? { kind: "后台 agent", name: agent, detail: summary } : { kind: "后台任务", name: null, detail: summary };
  const [, k, name, rest] = m;
  const code = /\(exit(?: code)? (\d+)\)/.exec(rest)?.[1];
  const reason = /^failed: ([\s\S]+)$/.exec(rest)?.[1];
  return {
    kind: agent ? "后台 agent" : KINDS[k]!,
    name: agent ?? name,
    detail: reason ? reason.trim() : code && code !== "0" ? `退出码 ${code}` : null,
  };
}
