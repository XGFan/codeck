import { format } from "date-fns";

/** 毫秒时间戳 -> "10:42"。 */
export const clock = (ms: number | null | undefined): string => (ms == null ? "" : format(ms, "HH:mm"));

/** A command prompt as typed: `/name args`, or `!line` for a `!` shell command. */
export const commandLine = (name: string, args: string): string =>
  name === "!" ? `!${args}` : `${name} ${args}`.trim();

/** Parses JSON text (a tool call's input, a result's extra); `null` when it is missing or does not parse. */
export function parseJson<T = Record<string, unknown>>(text: string | null | undefined): T | null {
  if (!text) return null;
  try {
    return JSON.parse(text) as T;
  } catch {
    return null;
  }
}

/** Pretty-prints JSON text; returns the input unchanged when it does not parse. */
export function prettyJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

/** "claude-sonnet-5-5" -> "sonnet"; unknown model names are returned unchanged. */
export function shortModel(model: string | null | undefined): string {
  if (!model) return "";
  return /(opus|sonnet|haiku)/i.exec(model)?.[1]?.toLowerCase() ?? model;
}
