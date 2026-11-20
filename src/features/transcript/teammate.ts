import type { Node, RelayedMessage } from "@/ipc/bindings";

// What a prompt relayed from other agents shows. The messages come parsed from the backend (`relayed` of a user
// prompt: sender, color, summary, body, preview, and what a teammate's JSON message says); here they are only
// arranged for display.

/** One message of a relayed prompt. */
export type TeammateMessage = RelayedMessage;

/** The sender's name where none is recorded. */
const UNKNOWN = "未知";

/**
 * What a relayed message says, part by part: reports (plain text, or a JSON message of a kind the backend does not
 * know) shown as they are; the end of a teammate's round (with its result, else just idle, or failed with the reason);
 * its shutdown (from it and from `system` naming it, usually together: one part); a task assigned.
 */
export type TeammatePart =
  | { kind: "report"; messages: TeammateMessage[] }
  | { kind: "done"; who: string; color: string | null; result: string; failed: boolean }
  | { kind: "idle"; who: string; color: string | null }
  | { kind: "failed"; who: string; color: string | null; reason: string | null }
  | { kind: "exited"; who: string; color: string | null }
  | { kind: "task"; who: string; color: string | null; taskId: string | null; subject: string | null };

function partOf(m: TeammateMessage): Exclude<TeammatePart, { kind: "report" }> | null {
  const e = m.event;
  if (!e) return null;
  const who = e.who ?? UNKNOWN;
  switch (e.kind) {
    case "done":
      return { kind: "done", who, color: m.color, result: e.result, failed: e.failed };
    case "idle":
      return { kind: "idle", who, color: m.color };
    case "failed":
      return { kind: "failed", who, color: m.color, reason: e.reason };
    case "exited":
      // `system`'s notice is in its own color, not the teammate's.
      return { kind: "exited", who, color: e.system ? null : m.color };
    case "task":
      return { kind: "task", who, color: m.color, taskId: e.taskId, subject: e.subject };
  }
}

/** The parts of a relayed message, in order; adjacent reports form one part. */
export function teammateParts(messages: readonly TeammateMessage[]): TeammatePart[] {
  const out: TeammatePart[] = [];
  for (const m of messages) {
    const p = partOf(m);
    const last = out[out.length - 1];
    if (!p) {
      if (last?.kind === "report") last.messages.push(m);
      else out.push({ kind: "report", messages: [m] });
      continue;
    }
    if (p.kind === "exited") {
      // Its shutdown_approved and the system's teammate_terminated: one line, in its color.
      const same = out.find(
        (o): o is Extract<TeammatePart, { kind: "exited" }> => o.kind === "exited" && o.who === p.who,
      );
      if (same) {
        same.color ??= p.color;
        continue;
      }
    }
    out.push(p);
  }
  return out;
}

/** One-line preview of a report. */
export const previewOf = (m: TeammateMessage): string => m.preview;

/**
 * What a relayed message shows, item by item: every report its own card; the end of a round with its result one
 * card, carrying the report the same teammate sent just before it (dropped when it says nearly the same); the
 * other events as they are.
 */
export type TeammateItem =
  | { kind: "report"; message: TeammateMessage }
  | (Extract<TeammatePart, { kind: "done" }> & { report: TeammateMessage | null })
  | Exclude<TeammatePart, { kind: "report" | "done" }>;

export function teammateItems(parts: TeammatePart[]): TeammateItem[] {
  const out: TeammateItem[] = [];
  parts.forEach((p, i) => {
    if (p.kind === "report") {
      const next = parts[i + 1];
      p.messages.forEach((m, j) => {
        // The report right before its sender's done goes with that card.
        if (j === p.messages.length - 1 && next?.kind === "done" && m.from === next.who) return;
        out.push({ kind: "report", message: m });
      });
    } else if (p.kind === "done") {
      const prev = parts[i - 1];
      const m = prev?.kind === "report" ? prev.messages[prev.messages.length - 1] : undefined;
      out.push({ ...p, report: m && m.from === p.who && !nearlySame(m.body, p.result) ? m : null });
    } else {
      out.push(p);
    }
  });
  return out;
}

const norm = (s: string) => s.replace(/[\s`*_#>|\-:：，。、；;,.!！?？()（）[\]]/g, "").toLowerCase();

/** Whether two texts say nearly the same (one contains the other, or their character pairs mostly match). */
export function nearlySame(a: string, b: string): boolean {
  const x = norm(a);
  const y = norm(b);
  if (!x || !y) return false;
  if (x.includes(y) || y.includes(x)) return true;
  const pairs = (s: string) => {
    const m = new Map<string, number>();
    for (let i = 0; i < s.length - 1; i++) m.set(s.slice(i, i + 2), (m.get(s.slice(i, i + 2)) ?? 0) + 1);
    return m;
  };
  const px = pairs(x);
  const py = pairs(y);
  let common = 0;
  for (const [g, n] of px) common += Math.min(n, py.get(g) ?? 0);
  return (2 * common) / (x.length - 1 + (y.length - 1)) >= 0.8;
}

/** Other Sessions' addresses → names, from the messages they sent (a SendMessage to one names its address). */
export function peerNames(nodes: readonly Node[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const n of nodes) {
    if (n.body.kind !== "userPrompt") continue;
    for (const m of n.body.relayed) if (m.address && m.from && m.from !== m.address) out.set(m.address, m.from);
  }
  return out;
}
