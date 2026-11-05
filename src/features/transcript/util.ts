import { format } from "date-fns";

/** 毫秒时间戳 -> "10:42"。 */
export const clock = (ms: number | null | undefined): string => (ms == null ? "" : format(ms, "HH:mm"));

/** A command prompt as typed: `/name args`, or `!line` for a `!` shell command. */
export const commandLine = (name: string, args: string): string =>
  name === "!" ? `!${args}` : `${name} ${args}`.trim();

/** Pretty-prints JSON text; returns the input unchanged when it does not parse. */
export function prettyJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

/** 毫秒 -> "42 秒"、"1 分 04 秒"、"1 小时 12 分"。 */
export function elapsed(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return "";
  const sec = Math.round(ms / 1000);
  if (sec < 60) return `${sec} 秒`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m} 分 ${String(sec % 60).padStart(2, "0")} 秒`;
  const h = Math.floor(m / 60);
  return m % 60 ? `${h} 小时 ${m % 60} 分` : `${h} 小时`;
}

/** "claude-sonnet-5-5" -> "sonnet"; unknown model names are returned unchanged. */
export function shortModel(model: string | null | undefined): string {
  if (!model) return "";
  return /(opus|sonnet|haiku)/i.exec(model)?.[1]?.toLowerCase() ?? model;
}
