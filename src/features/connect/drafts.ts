// Unsent input in a Session's input box, kept in localStorage across restarts.

import { withTokens } from "./imageTokens";

/** A pasted image waiting to be sent: its saved path and a preview `data:` URL. */
export interface DraftImage {
  path: string;
  url: string;
}

/** What is typed in a Session's input box and not sent yet: the text holds a token where each image goes (`imageTokens`). */
export interface Draft {
  text: string;
  images: DraftImage[];
}

export const EMPTY_DRAFT: Draft = { text: "", images: [] };

const DRAFTS_KEY = "codeck.connect.drafts";
/** Where drafts were kept before the rename to Connect: read when the new key is absent, removed on the next save. */
const OLD_DRAFTS_KEY = "codeck.drive.drafts";
// A Session's id; a connection's key dies with it, so only these are kept across restarts.
const SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const isDraft = (d: unknown): d is Draft =>
  !!d &&
  typeof (d as Draft).text === "string" &&
  Array.isArray((d as Draft).images) &&
  (d as Draft).images.every((i) => typeof i?.path === "string" && typeof i?.url === "string");

export function loadDrafts(): Record<string, Draft> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(DRAFTS_KEY) ?? localStorage.getItem(OLD_DRAFTS_KEY) ?? "{}");
    if (!v || typeof v !== "object") return {};
    return Object.fromEntries(
      Object.entries(v)
        .filter(([k, d]) => SESSION_ID.test(k) && isDraft(d))
        // A draft from before image tokens: its images go first, where they were sent then.
        .map(([k, d]) => [k, { ...(d as Draft), text: withTokens((d as Draft).text, (d as Draft).images) }]),
    );
  } catch {
    return {};
  }
}

let pendingDrafts: Record<string, Draft> | null = null;
let draftsTimer: ReturnType<typeof setTimeout> | undefined;

function flushDrafts() {
  clearTimeout(draftsTimer);
  const drafts = pendingDrafts;
  pendingDrafts = null;
  if (!drafts) return;
  const keep = Object.fromEntries(
    Object.entries(drafts).filter(([k, d]) => SESSION_ID.test(k) && (d.text.trim() || d.images.length)),
  );
  try {
    localStorage.removeItem(OLD_DRAFTS_KEY);
    localStorage.setItem(DRAFTS_KEY, JSON.stringify(keep));
  } catch {
    // Over the storage quota with the images' previews: keep the text at least.
    try {
      localStorage.setItem(
        DRAFTS_KEY,
        JSON.stringify(Object.fromEntries(Object.entries(keep).map(([k, d]) => [k, { text: d.text, images: [] }]))),
      );
    } catch {
      // Storage unavailable: the drafts last until the app quits.
    }
  }
}

/** Unsent input survives a restart: written shortly after it changes, and when the page goes away. */
export function saveDrafts(drafts: Record<string, Draft>) {
  pendingDrafts = drafts;
  clearTimeout(draftsTimer);
  draftsTimer = setTimeout(flushDrafts, 300);
}
if (typeof window !== "undefined") window.addEventListener("pagehide", flushDrafts);
