/** The message of a rejected IPC call (`AppError`), an `Error`, or anything else thrown. */
export const errorText = (e: unknown): string =>
  e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e);

/** A rejected IPC call that found nothing (`AppError` with code `notFound`). */
export const isNotFound = (e: unknown): boolean =>
  !!e && typeof e === "object" && (e as { code?: unknown }).code === "notFound";
