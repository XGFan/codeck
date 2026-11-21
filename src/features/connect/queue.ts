import type { Draft, DraftImage } from "./drafts";
import { token } from "./imageTokens";
import type { QueueItem } from "./protocol";

const firstLine = (s: string) =>
  s
    .split("\n")
    .find((l) => l.trim())
    ?.trim() ?? "";

/** A queued item as it reads in the queue notice: a message from elsewhere as `<sender>：<first line>`; what was typed as it is. */
export function queueText(item: QueueItem): string {
  return item.from ? `${item.from}：${firstLine(item.text)}` : item.text;
}

const IMAGE_TAG = /\[Image #\d+\] ?/g;
/** Most images kept: only those of items still in the queue are asked for. */
const MAX_SENT_IMAGES = 50;

/** Images sent from here, by path: the queue item has only the path and `[Image #n]`, the preview comes from here. */
const sentImages = new Map<string, DraftImage>();

/** Images went out with a prompt or steer: kept to give them back should ↑ take it from the queue. */
export function rememberImages(images: DraftImage[]) {
  for (const i of images) {
    sentImages.delete(i.path);
    sentImages.set(i.path, i);
  }
  for (const path of [...sentImages.keys()].slice(0, Math.max(0, sentImages.size - MAX_SENT_IMAGES)))
    sentImages.delete(path);
}

/**
 * What `edit_queue` took back, as a draft: the items one per line, oldest first. An item's `[Image #n]` are its
 * `images` in order: one sent from here comes back with its image, any other (path unknown here, or ccrpc does not
 * know the paths) is left out, and an item that was only such images is gone.
 */
export function takenBack(items: QueueItem[]): Draft {
  const lines: string[] = [];
  const images: Draft["images"] = [];
  for (const item of items) {
    let at = 0;
    const line = item.text.replace(IMAGE_TAG, () => {
      const path = item.images?.[at++];
      const image = path ? sentImages.get(path) : undefined;
      if (!image) return "";
      images.push(image);
      return token(images.length);
    });
    if (line.trim()) lines.push(line);
  }
  return { text: lines.join("\n"), images };
}
