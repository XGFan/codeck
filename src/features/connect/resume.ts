// Resume (CONTEXT.md): start an ended Session again in a new clay Pane (`claude --resume`) and Connect it.

/** What the header offers for Resume. */
export type ResumeOffer = { kind: "hidden" } | { kind: "enabled" } | { kind: "disabled"; why: string };

/**
 * Resume is for an ended Session: one still running (in a terminal outside clay, or not yet in `ccrpc ls`) is not
 * started a second time, which would have two claude processes write one transcript. It needs the directory the
 * Session started in, and ccrpc.
 */
export function resumeOffer(o: { live: boolean; cwd: string | null; cwdMissing: boolean; ccrpcError: string | null }): ResumeOffer {
  if (o.live) return { kind: "hidden" };
  if (!o.cwd) return { kind: "disabled", why: "不知道这个 Session 的启动目录" };
  if (o.cwdMissing) return { kind: "disabled", why: `目录 ${o.cwd} 已不存在` };
  if (o.ccrpcError) return { kind: "disabled", why: o.ccrpcError };
  return { kind: "enabled" };
}

/** How claude reports a Session it cannot resume (2.1.318: `No conversation found with session ID: <id>`). */
const CLAUDE_ERROR = /^(No conversation found|Error:|error:)/;

/**
 * Why a Resume failed, from the Pane's screen once claude exited: the last error line claude printed, else the last
 * non-empty line above the cursor (the shell's prompt that came back, of however many lines, starts below it).
 */
export function resumeFailure(lines: string[], cursorRow: number | null): string {
  const rows = lines.map((l) => l.trim());
  const error = rows.filter((l) => CLAUDE_ERROR.test(l)).at(-1);
  if (error) return error;
  const above = (cursorRow != null ? rows.slice(0, cursorRow) : rows.slice(0, -1)).filter(Boolean);
  return above.at(-1) ?? rows.filter(Boolean).at(-1) ?? "claude 退出了";
}
