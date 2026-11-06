import type { LsEntry } from "@/features/connect/protocol";
import type { ProjectSummary } from "@/ipc/bindings";

/** A Session in `ccrpc ls` that is not in the index yet (no transcript before its first prompt): a placeholder row. */
export interface PendingSession {
  id: string;
  cwd: string | null;
  status: "busy" | "waiting" | "idle";
}

/** The Sessions of Forks under way (`useConnect`'s `forking`): no placeholder rows for them. */
export const forkingSessions = (forking: Record<string, { sessionId: string | null }>) =>
  Object.values(forking).flatMap((f) => (f.sessionId ? [f.sessionId] : []));

/** The Project a cwd is in: the deepest Project whose path is the cwd or one of its parents. */
export function projectOfCwd(cwd: string | null, projects: ProjectSummary[]): string | null {
  if (!cwd) return null;
  let best: ProjectSummary | null = null;
  for (const p of projects) {
    const inside = cwd === p.path || cwd.startsWith(p.path.endsWith("/") ? p.path : `${p.path}/`);
    if (inside && (!best || p.path.length > best.path.length)) best = p;
  }
  return best?.id ?? null;
}

/**
 * The placeholder rows: `ccrpc ls` entries whose Session is not in `indexed` (every indexed Session id; null while
 * unknown, then none). Under a Project filter only those whose cwd is in a filtered Project; none under a stats drill.
 */
export function pendingSessions(
  ls: LsEntry[],
  indexed: ReadonlySet<string> | null,
  filter: { projectIds: string[]; drill: boolean },
  projects: ProjectSummary[],
): PendingSession[] {
  if (!indexed || filter.drill) return [];
  const out: PendingSession[] = [];
  const seen = new Set<string>();
  for (const e of ls) {
    const id = e.session_id;
    if (!id || indexed.has(id) || seen.has(id)) continue;
    if (filter.projectIds.length > 0) {
      const p = projectOfCwd(e.cwd, projects);
      if (!p || !filter.projectIds.includes(p)) continue;
    }
    seen.add(id);
    out.push({ id, cwd: e.cwd, status: e.status === "busy" ? "busy" : e.status === "waiting" ? "waiting" : "idle" });
  }
  return out;
}
