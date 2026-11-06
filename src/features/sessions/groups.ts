import type { LiveDotState } from "@/components/LiveDot";
import type { ProjectSummary, SessionSummary, SidebarProject } from "@/ipc/bindings";
import { projectOfCwd, type PendingSession } from "./pending";

/** A row of the sidebar: an indexed Session, or one in `ccrpc ls` not indexed yet (`pending`). */
export interface SideSession {
  id: string;
  title: string;
  /** The latest user input (`lastPromptMs`); a pending Session counts as now. */
  timeMs: number;
  live: LiveDotState | null;
  pending: boolean;
  /** The Project it started in (for a pending Session, the one its cwd is in); null when none is known. */
  projectId: string | null;
  /** A pending Session's cwd. */
  cwd: string | null;
}

/** The Sessions of one Project gathered into a nesting group (「收拢子 Project」). */
export interface SubGroup {
  projectId: string;
  name: string;
  sessions: SideSession[];
  lastMs: number;
  /** A Session of it had a prompt in the last 30 days: listed on its own, the rest under 「其他 N 个子 Project」. */
  active: boolean;
}

/** An added Project. */
export interface Group {
  path: string;
  name: string;
  nest: boolean;
  /** Sessions started in the Project itself (or, without nesting, in it only). */
  sessions: SideSession[];
  /** With `nest`: the Projects under it that have Sessions, latest first. */
  subs: SubGroup[];
  total: number;
  /** The latest Session's time; 0 for a group without Sessions. */
  lastMs: number;
  /** The most urgent Live state among its Sessions: waiting, busy, idle. */
  live: LiveDotState | null;
  /** Indexed Projects under its path (what 「收拢子 Project」 would gather). */
  childProjects: number;
}

export interface Sidebar {
  /** Every Live Session (and pending one), latest first: the 「Live」 list. */
  live: SideSession[];
  /** The added Projects, in their order. */
  groups: Group[];
  /** Every indexed Session that is not Live, in an added Project or not, latest first: the 「Recent」 list. */
  recent: SideSession[];
}

const ACTIVE_MS = 30 * 24 * 3600 * 1000;
const URGENCY: Record<LiveDotState, number> = { waiting: 3, busy: 2, idle: 1 };

export const isUnder = (path: string, dir: string) =>
  path.startsWith(dir.endsWith("/") ? dir : `${dir}/`) && path !== dir;

function liveOf(s: SessionSummary): LiveDotState | null {
  if (!s.live) return null;
  return s.live.status === "busy" ? "busy" : s.live.status === "waiting" ? "waiting" : "idle";
}

const byTime = (a: SideSession, b: SideSession) => b.timeMs - a.timeMs || a.id.localeCompare(b.id);

const basename = (path: string) => path.split("/").filter(Boolean).pop() ?? path;

/** The added Project a directory belongs to: the deepest one that is it, or (nesting) one of its parents. */
export function entryFor(path: string, entries: SidebarProject[]): SidebarProject | null {
  let best: SidebarProject | null = null;
  for (const e of entries) {
    if ((path === e.path || (e.nest && isUnder(path, e.path))) && (!best || e.path.length > best.path.length)) best = e;
  }
  return best;
}

/** Indexed Projects under `path` (not `path` itself). */
export const childProjects = (path: string, projects: ProjectSummary[]) =>
  projects.filter((p) => isUnder(p.path, path)).length;

/**
 * The sidebar: Sessions grouped by the added Project they started in (only once each; groups in the order the user
 * gave them), the Live ones listed again under 「Live」, the others again under 「Recent」.
 */
export function buildSidebar(
  sessions: SessionSummary[],
  pending: PendingSession[],
  projects: ProjectSummary[],
  entries: SidebarProject[],
  now: number,
): Sidebar {
  const byId = new Map(projects.map((p) => [p.id, p]));
  const rows: { s: SideSession; path: string | null }[] = [
    ...pending.map((p) => {
      const projectId = projectOfCwd(p.cwd, projects);
      return {
        s: {
          id: p.id,
          title: "新 Session",
          timeMs: now,
          live: p.status,
          pending: true,
          projectId,
          cwd: p.cwd,
        },
        path: (projectId ? byId.get(projectId)?.path : null) ?? p.cwd,
      };
    }),
    ...sessions.map((s) => ({
      s: {
        id: s.id,
        title: s.title,
        timeMs: s.lastPromptMs,
        live: liveOf(s),
        pending: false,
        projectId: s.projectId,
        cwd: null,
      },
      path: byId.get(s.projectId)?.path ?? null,
    })),
  ];

  const groups = new Map<string, Group & { subMap: Map<string, SubGroup> }>(
    entries.map((e) => [
      e.path,
      {
        path: e.path,
        name: projects.find((p) => p.path === e.path)?.displayName ?? basename(e.path),
        nest: e.nest ?? false,
        sessions: [],
        subs: [],
        total: 0,
        lastMs: 0,
        live: null,
        childProjects: childProjects(e.path, projects),
        subMap: new Map(),
      },
    ]),
  );
  for (const { s, path } of rows) {
    const e = path ? entryFor(path, entries) : null;
    const g = e && groups.get(e.path);
    if (!g) continue;
    g.total++;
    g.lastMs = Math.max(g.lastMs, s.timeMs);
    if (s.live && (!g.live || URGENCY[s.live] > URGENCY[g.live])) g.live = s.live;
    if (path === g.path || !s.projectId) {
      g.sessions.push(s);
      continue;
    }
    let sub = g.subMap.get(s.projectId);
    if (!sub) {
      sub = {
        projectId: s.projectId,
        name: byId.get(s.projectId)?.displayName ?? basename(path ?? ""),
        sessions: [],
        lastMs: 0,
        active: false,
      };
      g.subMap.set(s.projectId, sub);
    }
    sub.sessions.push(s);
    sub.lastMs = Math.max(sub.lastMs, s.timeMs);
  }

  const list: Group[] = [...groups.values()].map(({ subMap, ...g }) => {
    g.sessions.sort(byTime);
    const subs = [...subMap.values()];
    for (const sub of subs) {
      sub.sessions.sort(byTime);
      sub.active = sub.lastMs >= now - ACTIVE_MS;
    }
    subs.sort((a, b) => b.lastMs - a.lastMs || a.name.localeCompare(b.name));
    return { ...g, subs };
  });
  const all = rows.map((r) => r.s);
  return {
    live: all.filter((s) => s.live).sort(byTime),
    groups: list,
    recent: all.filter((s) => !s.live && !s.pending).sort(byTime),
  };
}

/** `entries` with the one at `from` moved to insertion point `to` (0…length, counted before the move). */
export function moveEntry<T>(entries: T[], from: number, to: number): T[] {
  if (from < 0 || from >= entries.length) return entries;
  const out = entries.filter((_, i) => i !== from);
  out.splice(to > from ? to - 1 : to, 0, entries[from]!);
  return out;
}

export const ROW_PX = 28;
export const LIVE_ROWS = 5;
/** 「Live」's heading and padding; 「Recent」's heading, padding and top border. */
export const LIVE_CHROME = 34;
export const RECENT_CHROME = 37;
/** The Project list keeps its heading (32), 4 rows, its bottom padding and the border above it. */
const MIDDLE_MIN = 32 + 4 * ROW_PX + 10 + 1;

/**
 * Rows shown by 「Live」 and 「Recent」 in a column `height` px tall (between 「新建 Session」 and the foot): up to 5
 * and `recentRows` (the user's, 6 at first), then fewer — 「Recent」 first, down to one each — while the Project
 * list would get less than its heading and 4 rows. A list with nothing in it is not shown.
 */
export function fitRows(
  height: number,
  live: number,
  recent: number,
  recentOpen: boolean,
  recentRows = 6,
): { live: number; recent: number } {
  let l = Math.min(live, LIVE_ROWS);
  let r = recentOpen ? Math.min(recent, recentRows) : 0;
  // Collapsed, 「Recent」 is its heading and border alone (no list padding).
  const recentH = () => (!recent ? 0 : recentOpen ? RECENT_CHROME + r * ROW_PX : RECENT_CHROME - 4);
  const used = () => (live ? LIVE_CHROME + l * ROW_PX : 0) + recentH() + MIDDLE_MIN;
  while (used() > height && r > 1) r--;
  while (used() > height && l > 1) l--;
  return { live: l, recent: r };
}

/** The most rows 「Recent」 can be dragged to: what leaves the Project list its heading and 4 rows. */
export function recentRoom(height: number, liveShown: number): number {
  const live = liveShown ? LIVE_CHROME + liveShown * ROW_PX : 0;
  return Math.max(1, Math.floor((height - live - RECENT_CHROME - MIDDLE_MIN) / ROW_PX));
}

/** Rows a list shows before 「显示更多」, and how many more each click adds. */
export const FIRST_ROWS = 5;
export const MORE_ROWS = 20;

export const groupKey = (path: string) => `g:${path}`;
export const subKey = (path: string, projectId: string) => `s:${path}\n${projectId}`;
export const restKey = (path: string) => `r:${path}`;
/** 「Recent」 open or collapsed (in the same remembered map as the groups). */
export const RECENT_KEY = "recent";

/** Groups and 「Recent」 start open; sub-groups and 「其他 N 个子 Project」 start closed. */
export const isOpen = (expanded: Record<string, boolean>, key: string) =>
  expanded[key] ?? (key.startsWith("g:") || key === RECENT_KEY);

export type Item =
  | { kind: "section"; key: string }
  | { kind: "group"; key: string; g: Group; open: boolean; index: number }
  | { kind: "row"; key: string; s: SideSession; depth: 1 | 2 }
  | { kind: "more"; key: string; list: string; rest: number; depth: 1 | 2 }
  | { kind: "sub"; key: string; sub: SubGroup; open: boolean }
  | { kind: "subsRest"; key: string; count: number; open: boolean };

/**
 * The Project list's rows top to bottom, under the open / 「显示更多」 state (`limits`: rows shown per list key);
 * `headersOnly` (while a group is dragged) lists the group headers alone.
 */
export function flatten(
  sb: Sidebar,
  expanded: Record<string, boolean>,
  limits: Record<string, number>,
  headersOnly = false,
): Item[] {
  const out: Item[] = [];
  const list = (key: string, rows: SideSession[], depth: 1 | 2) => {
    const n = limits[key] ?? FIRST_ROWS;
    for (const s of rows.slice(0, n)) out.push({ kind: "row", key: `${key}\n${s.id}`, s, depth });
    if (rows.length > n) out.push({ kind: "more", key: `${key}\nmore`, list: key, rest: rows.length - n, depth });
  };

  out.push({ kind: "section", key: "sec:projects" });
  sb.groups.forEach((g, index) => {
    const gk = groupKey(g.path);
    const open = !headersOnly && isOpen(expanded, gk);
    out.push({ kind: "group", key: gk, g, open, index });
    if (!open) return;
    list(gk, g.sessions, 1);
    const active = g.subs.filter((s) => s.active);
    const rest = g.subs.filter((s) => !s.active);
    const sub = (s: SubGroup) => {
      const sk = subKey(g.path, s.projectId);
      const subOpen = isOpen(expanded, sk);
      out.push({ kind: "sub", key: sk, sub: s, open: subOpen });
      if (subOpen) list(sk, s.sessions, 2);
    };
    active.forEach(sub);
    if (rest.length > 0) {
      const rk = restKey(g.path);
      const restOpen = isOpen(expanded, rk);
      out.push({ kind: "subsRest", key: rk, count: rest.length, open: restOpen });
      if (restOpen) rest.forEach(sub);
    }
  });
  return out;
}

/**
 * What to open, and how many rows to show, for Session `id` to have a row in the Project list (it was opened from
 * ⌘K, a link, …); null when it has none there or already has one.
 */
export function reveal(
  sb: Sidebar,
  id: string,
  expanded: Record<string, boolean>,
  limits: Record<string, number>,
): { open: string[]; limits: Record<string, number> } | null {
  const where = (): { open: string[]; key: string; index: number } | null => {
    const at = (open: string[], key: string, rows: SideSession[]) => {
      const index = rows.findIndex((s) => s.id === id);
      return index < 0 ? null : { open, key, index };
    };
    for (const g of sb.groups) {
      const gk = groupKey(g.path);
      const direct = at([gk], gk, g.sessions);
      if (direct) return direct;
      for (const s of g.subs) {
        const sk = subKey(g.path, s.projectId);
        const r = at(s.active ? [gk, sk] : [gk, restKey(g.path), sk], sk, s.sessions);
        if (r) return r;
      }
    }
    return null;
  };
  const w = where();
  if (!w) return null;
  const open = w.open.filter((k) => !isOpen(expanded, k));
  const more = w.index >= (limits[w.key] ?? FIRST_ROWS) ? { [w.key]: w.index + 1 } : {};
  return open.length > 0 || Object.keys(more).length > 0 ? { open, limits: more } : null;
}

/**
 * Projects the first 「添加 Project」 ticks: active in the last 7 days, still on disk, and not a parent of other
 * Projects (a home or workspace folder would swallow them).
 */
export function suggested(projects: ProjectSummary[], now: number): Set<string> {
  return new Set(
    projects
      .filter(
        (p) => !p.missing && p.lastActiveMs >= now - 7 * 24 * 3600 * 1000 && childProjects(p.path, projects) === 0,
      )
      .map((p) => p.path),
  );
}
