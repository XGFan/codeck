import { peerMessages, previewOf, teammateMessages } from "@/features/transcript/teammate";
import type { Draft } from "./drafts";
import { shiftTokens, withoutTokens } from "./imageTokens";

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

/** A queued item ↑ takes back into the input box (`edit_queue`): what was typed, not a message from elsewhere. */
export function editable(item: string): boolean {
  return !/^<(cross-session-message|agent-message|teammate-message|task-notification)\b/.test(item.trimStart());
}

const IMAGE_TAG = /\[Image #\d+\]/g;
/** What a steer and its queued item have in common: the words without the images and whitespace, and how many images. */
const keyOf = (words: string, images: number) => `${images}:${words.replace(/\s+/g, "")}`;

/** Steers sent from here with images, by connection: the queue has only their `[Image #n]`, the images come from here. */
const withImages = new Map<string, { key: string; draft: Draft }[]>();

/** A steer was queued: if it has images, kept to give them back should it be taken back. */
export function rememberSteer(conn: string, draft: Draft) {
  if (draft.images.length === 0) return;
  const key = keyOf(withoutTokens(draft.text), draft.images.length);
  const kept = (withImages.get(conn) ?? []).filter((s) => s.key !== key);
  withImages.set(conn, [...kept, { key, draft }].slice(-20));
}

/** The connection is gone: what it kept goes too. */
export function forgetSteers(conn: string) {
  withImages.delete(conn);
}

/**
 * What `edit_queue` took back, as a draft: the items one per line, oldest first. One sent from here with images comes
 * back as it was typed, its images too (taken: a later one with the same words is another); elsewhere an image's
 * `[Image #n]` is left out (the image is not to be had), and an item that was only images is gone.
 */
export function takenBack(conn: string, items: string[]): Draft {
  const sent = withImages.get(conn) ?? [];
  const lines: string[] = [];
  const images: Draft["images"] = [];
  for (const item of items) {
    const key = keyOf(item.replace(IMAGE_TAG, ""), item.match(IMAGE_TAG)?.length ?? 0);
    const i = sent.findIndex((s) => s.key === key);
    if (i >= 0) {
      const [{ draft }] = sent.splice(i, 1);
      lines.push(shiftTokens(draft.text, images.length));
      images.push(...draft.images);
    } else {
      const words = item.replace(/\[Image #\d+\] ?/g, "");
      if (words.trim()) lines.push(words);
    }
  }
  return { text: lines.join("\n"), images };
}
