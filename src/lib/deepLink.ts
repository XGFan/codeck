/** What a `codeck://` link asks for. Links only navigate: none starts, sends or changes anything. */
export type DeepLink = { kind: "session"; sessionId: string };

const SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** `codeck://session/<sessionId>`; anything else is null. Unknown query parameters are ignored. */
export function parseDeepLink(raw: string): DeepLink | null {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== "codeck:" || url.host !== "session") return null;
  const id = url.pathname.replace(/^\/|\/$/g, "");
  return SESSION_ID.test(id) ? { kind: "session", sessionId: id.toLowerCase() } : null;
}
