import { peerMessages, previewOf, teammateMessages } from "@/features/transcript/teammate";

const firstLine = (s: string) =>
  s
    .split("\n")
    .find((l) => l.trim())
    ?.trim() ?? "";

/**
 * A queued item as it reads in the queue notice: a message from another agent or a Teammate as `<sender>：<first
 * line>`, a background task's notification as `后台任务：<summary>`; anything else as it was typed.
 */
export function queueText(item: string): string {
  const t = item.trim();
  if (/^<(cross-session-message|agent-message)\b/.test(t)) {
    const m = peerMessages(t, null)[0]!;
    return `${m.id ?? "Agent"}：${firstLine(m.body)}`;
  }
  if (t.startsWith("<teammate-message")) {
    const m = teammateMessages(t)[0]!;
    return `${m.id ?? "Teammate"}：${previewOf(m)}`;
  }
  if (t.startsWith("<task-notification>")) {
    const summary = /<summary>([\s\S]*?)<\/summary>/.exec(t)?.[1]?.trim();
    return summary ? `后台任务：${summary}` : "后台任务通知";
  }
  return item;
}
