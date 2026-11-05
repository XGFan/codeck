import type { ProjectSummary, SessionSummary } from "@/ipc/bindings";

/** Projects of a session in display order: the one it started in, then the others it edited. */
export function sessionProjectIds(s: SessionSummary): string[] {
  return [s.projectId, ...s.editedProjectIds.filter((id) => id !== s.projectId)];
}

/** Lower-cased literal terms; phrases keep their inner spaces. */
export interface ParsedQuery {
  include: string[];
  exclude: string[];
}

/**
 * The full-text search's syntax (`search/query.rs`): whitespace AND, `"phrase"` (to the next `"` or the
 * end), a leading `-` excludes a word or phrase.
 */
export function parseQuery(query: string): ParsedQuery {
  const out: ParsedQuery = { include: [], exclude: [] };
  let rest = query;
  for (;;) {
    rest = rest.trimStart();
    if (!rest) return out;
    const negate = rest.startsWith("-") && rest.length > 1 && !/\s/.test(rest[1]!);
    if (negate) rest = rest.slice(1);
    let term: string;
    if (rest.startsWith('"')) {
      const end = rest.indexOf('"', 1);
      term = rest.slice(1, end < 0 ? undefined : end);
      rest = end < 0 ? "" : rest.slice(end + 1);
    } else {
      const end = rest.search(/\s/);
      term = rest.slice(0, end < 0 ? undefined : end);
      rest = end < 0 ? "" : rest.slice(end);
    }
    if (term) (negate ? out.exclude : out.include).push(term.toLowerCase());
  }
}

/** Every included term in some field, no excluded term in any; nothing matches without an included term. */
function matches(fields: string[], q: ParsedQuery): boolean {
  if (q.include.length === 0) return false;
  return (
    q.include.every((t) => fields.some((f) => f.includes(t))) &&
    !q.exclude.some((t) => fields.some((f) => f.includes(t)))
  );
}

/**
 * Sessions matching on the title, a Project name or the git branch. Sessions with more terms in a Project
 * name or the branch come first; ties keep the input order (newest first).
 */
export function matchSessions(
  sessions: SessionSummary[],
  names: Map<string, string>,
  q: ParsedQuery,
): SessionSummary[] {
  const scored: { s: SessionSummary; score: number }[] = [];
  for (const s of sessions) {
    const tags = [...sessionProjectIds(s).map((id) => names.get(id) ?? ""), s.gitBranch ?? ""].map((t) =>
      t.toLowerCase(),
    );
    if (!matches([s.title.toLowerCase(), ...tags], q)) continue;
    scored.push({ s, score: q.include.filter((t) => tags.some((tag) => tag.includes(t))).length });
  }
  return scored.sort((a, b) => b.score - a.score).map((x) => x.s);
}

/** Projects matching on the name or path, most recently active first. */
export function matchProjects(projects: ProjectSummary[], q: ParsedQuery): ProjectSummary[] {
  return projects
    .filter((p) => matches([p.displayName.toLowerCase(), p.path.toLowerCase()], q))
    .sort((a, b) => b.lastActiveMs - a.lastActiveMs);
}

/** `text` split into runs, `hit` where it contains one of `terms` (case-insensitive). */
export function highlight(text: string, terms: string[]): { text: string; hit: boolean }[] {
  // Longest first, so a term containing another wins the alternation.
  const ts = terms.filter(Boolean).sort((a, b) => b.length - a.length);
  if (ts.length === 0) return [{ text, hit: false }];
  const re = new RegExp(`(${ts.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "gi");
  return text
    .split(re)
    .map((part, i) => ({ text: part, hit: i % 2 === 1 }))
    .filter((p) => p.text);
}
