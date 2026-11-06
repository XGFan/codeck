/** One `<teammate-message>` of a message relayed from an agent team. */
export interface TeammateMessage {
  id: string | null;
  color: string | null;
  summary: string | null;
  body: string;
}

/**
 * What a relayed message says, part by part: reports (plain text, or a JSON message of a kind not known here) shown as
 * they are; the end of a teammate's round (`idle_notification`: with its `result` since claude 2.1.251, else just
 * idle; `idleReason: "failed"` with `failureReason`); its shutdown (`shutdown_approved` from it, `teammate_terminated`
 * from `system` naming it, usually together: one part); a task assigned (`task_assignment`).
 */
export type TeammatePart =
  | { kind: "report"; messages: TeammateMessage[] }
  | { kind: "done"; who: string; color: string | null; result: string; failed: boolean }
  | { kind: "idle"; who: string; color: string | null }
  | { kind: "failed"; who: string; color: string | null; reason: string | null }
  | { kind: "exited"; who: string; color: string | null }
  | { kind: "task"; who: string; color: string | null; taskId: string | null; subject: string | null };

const attr = (attrs: string, name: string) => new RegExp(`\\b${name}="([^"]*)"`).exec(attrs)?.[1]?.trim() || null;

/** The `<teammate-message>` blocks of a relayed message (the injected notice after them is dropped). */
export function teammateMessages(text: string): TeammateMessage[] {
  const out = [...text.matchAll(/<teammate-message\b([^>]*)>([\s\S]*?)<\/teammate-message>/g)].map((m) => ({
    id: attr(m[1], "teammate_id"),
    color: attr(m[1], "color"),
    summary: attr(m[1], "summary"),
    body: m[2].trim(),
  }));
  return out.length ? out : [{ id: null, color: null, summary: null, body: text.trim() }];
}

function jsonObject(body: string): Record<string, unknown> | null {
  if (!body.startsWith("{")) return null;
  try {
    const v: unknown = JSON.parse(body);
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

const str = (v: unknown) => (typeof v === "string" && v.trim() ? v.trim() : null);

function partOf(m: TeammateMessage): Exclude<TeammatePart, { kind: "report" }> | null {
  const v = jsonObject(m.body);
  if (!v) return null;
  const who = str(v.from) ?? m.id ?? "未知";
  switch (v.type) {
    case "idle_notification": {
      const failed = v.idleReason === "failed";
      const result = str(v.result);
      if (result) return { kind: "done", who, color: m.color, result, failed };
      if (failed) return { kind: "failed", who, color: m.color, reason: str(v.failureReason) };
      return { kind: "idle", who, color: m.color };
    }
    case "shutdown_approved":
      return { kind: "exited", who, color: m.color };
    case "teammate_terminated": {
      const name = /^(.+?) has shut down\.?$/.exec(str(v.message) ?? "")?.[1];
      return name ? { kind: "exited", who: name, color: null } : null;
    }
    case "task_assignment":
      return {
        kind: "task",
        who: str(v.assignedBy) ?? m.id ?? "未知",
        color: m.color,
        taskId: str(v.taskId),
        subject: str(v.subject),
      };
    default:
      return null;
  }
}

/** The parts of a relayed message, in order; adjacent reports form one part. */
export function teammateParts(text: string): TeammatePart[] {
  const out: TeammatePart[] = [];
  for (const m of teammateMessages(text)) {
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

/** One-line preview of a report: its `summary`, a JSON message's `summary` (else its `type`), else its first line. */
export function previewOf(m: TeammateMessage): string {
  if (m.summary) return m.summary;
  const v = jsonObject(m.body);
  if (v) return str(v.summary) ?? str(v.type) ?? "";
  return (
    m.body
      .split("\n")
      .find((l) => l.trim())
      ?.trim() ?? ""
  );
}
