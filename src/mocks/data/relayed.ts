// Mock only: what the backend gives as a relayed prompt's `relayed` (`relayed_messages` in
// crates/transcript/src/assemble/prompt.rs, where the format is parsed and tested), so mock data and test input can
// be written as the recorded text. The app itself never parses these tags.

import type { PromptOrigin, RelayedMessage, TeammateEvent } from "@/ipc/bindings";

const attr = (attrs: string, name: string) => new RegExp(`\\b${name}="([^"]*)"`).exec(attrs)?.[1]?.trim() || null;
const str = (v: unknown) => (typeof v === "string" && v.trim() ? v.trim() : null);

function jsonObject(body: string): Record<string, unknown> | null {
  if (!body.startsWith("{")) return null;
  try {
    const v: unknown = JSON.parse(body);
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

function previewOf(summary: string | null, body: string): string {
  if (summary) return summary;
  const v = jsonObject(body);
  if (v) return str(v.summary) ?? str(v.type) ?? "";
  return (
    body
      .split("\n")
      .find((l) => l.trim())
      ?.trim() ?? ""
  );
}

function eventOf(teammate: string | null, body: string): TeammateEvent | null {
  const v = jsonObject(body);
  if (!v) return null;
  const who = str(v.from) ?? teammate;
  switch (v.type) {
    case "idle_notification": {
      const failed = v.idleReason === "failed";
      const result = str(v.result);
      if (result) return { kind: "done", who, result, failed };
      if (failed) return { kind: "failed", who, reason: str(v.failureReason) };
      return { kind: "idle", who };
    }
    case "shutdown_approved":
      return { kind: "exited", who, system: false };
    case "teammate_terminated": {
      const name = /^(.+?) has shut down\.?$/.exec(str(v.message) ?? "")?.[1];
      return name ? { kind: "exited", who: name, system: true } : null;
    }
    case "task_assignment":
      return { kind: "task", who: str(v.assignedBy) ?? teammate, taskId: str(v.taskId), subject: str(v.subject) };
    default:
      return null;
  }
}

const plain = (from: string | null, body: string): RelayedMessage => ({
  from,
  address: null,
  color: null,
  summary: null,
  body,
  preview: previewOf(null, body),
  event: null,
});

export function relayedOf(origin: PromptOrigin, text: string): RelayedMessage[] {
  if (origin.kind === "teammate") {
    const out = [...text.matchAll(/<teammate-message\b([^>]*)>([\s\S]*?)<\/teammate-message>/g)].map((m) => {
      const from = attr(m[1]!, "teammate_id");
      const summary = attr(m[1]!, "summary");
      const body = m[2]!.trim();
      return {
        from,
        address: null,
        color: attr(m[1]!, "color"),
        summary,
        body,
        preview: previewOf(summary, body),
        event: eventOf(from, body),
      };
    });
    return out.length ? out : [plain(null, text.trim())];
  }
  if (origin.kind === "peer") {
    const out = [...text.matchAll(/<(cross-session-message|agent-message)\b([^>]*)>([\s\S]*?)<\/\1>/g)].map((m) => ({
      ...plain(attr(m[2]!, "from-name") ?? attr(m[2]!, "from") ?? origin.from, m[3]!.trim()),
      address: m[1] === "cross-session-message" ? attr(m[2]!, "from") : null,
    }));
    if (out.length) return out;
    return [plain(origin.from, text.trim().replace(/^The .+ plugin sent a message:\s*/, ""))];
  }
  return [];
}
