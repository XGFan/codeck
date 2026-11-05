/** The message of a rejected IPC call (`AppError`), an `Error`, or anything else thrown. */
export const errorText = (e: unknown): string =>
  e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e);
