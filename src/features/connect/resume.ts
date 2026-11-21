// Resume (CONTEXT.md): start an ended Session again in a new clay Pane (`claude --resume`) and Connect it.

/** What the header offers for Resume. */
export type ResumeOffer = { kind: "hidden" } | { kind: "enabled" } | { kind: "disabled"; why: string };

/**
 * Resume is for an ended Session: one still running (in a terminal outside clay, or not yet in `ccrpc ls`) is not
 * started a second time, which would have two claude processes write one transcript. It needs the directory the
 * Session started in, and ccrpc.
 */
export function resumeOffer(o: {
  live: boolean;
  cwd: string | null;
  cwdMissing: boolean;
  ccrpcError: string | null;
}): ResumeOffer {
  if (o.live) return { kind: "hidden" };
  if (!o.cwd) return { kind: "disabled", why: "不知道这个 Session 的启动目录" };
  if (o.cwdMissing) return { kind: "disabled", why: `目录 ${o.cwd} 已不存在` };
  if (o.ccrpcError) return { kind: "disabled", why: o.ccrpcError };
  return { kind: "enabled" };
}
