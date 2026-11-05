import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { api } from "@/ipc";
import type {
  FindRequest,
  ImageRequest,
  SearchRequest,
  SessionQuery,
  StatsRequest,
  ToolDetailRequest,
  TranscriptRequest,
} from "@/ipc/bindings";
import { MAIN_SCOPE, useUi } from "@/state/ui";

export const queryKeys = {
  appInfo: ["appInfo"] as const,
  indexStatus: ["indexStatus"] as const,
  projects: ["projects"] as const,
  sessions: (q?: SessionQuery) => (q ? (["sessions", q] as const) : (["sessions"] as const)),
  session: (id: string) => ["session", id] as const,
  /** `[ "transcript", sessionId, req ]`: events invalidate by session id prefix. */
  transcript: (req: TranscriptRequest) => ["transcript", req.sessionId, req] as const,
  toolDetail: (req: ToolDetailRequest) => ["toolDetail", req.sessionId, req] as const,
  /** A tool result's image is found by its tool call: parallel calls' results share the assistant node. */
  image: (req: ImageRequest) =>
    ["image", req.sessionId, req.image.nodeId, req.image.toolUseId, req.image.ordinal, req.scope] as const,
  search: (req: SearchRequest) => ["search", req] as const,
  find: (req: FindRequest) => ["find", req.sessionId, req] as const,
  stats: (req: StatsRequest) => ["stats", req] as const,
  diagnostics: ["diagnostics"] as const,
  slashCommands: (cwd: string | null) => ["slashCommands", cwd] as const,
  dirCompletions: (cwd: string | null, partial: string) => ["dirCompletions", cwd, partial] as const,
};

export const useAppInfo = () => useQuery({ queryKey: queryKeys.appInfo, queryFn: () => api.getAppInfo() });

/** Kept fresh by `useBackendEvents` (setQueryData on each event). */
export const useIndexStatus = () =>
  useQuery({ queryKey: queryKeys.indexStatus, queryFn: () => api.getIndexStatus(), staleTime: Infinity });

export const useProjects = () => useQuery({ queryKey: queryKeys.projects, queryFn: () => api.listProjects() });

export const useSessions = (query: SessionQuery, enabled = true) =>
  useQuery({
    queryKey: queryKeys.sessions(query),
    queryFn: () => api.listSessions(query),
    placeholderData: keepPreviousData,
    enabled,
  });

const ALL_SESSIONS: SessionQuery = {
  projectIds: [],
  sort: "lastActive",
  descending: true,
  timeRange: null,
  drill: null,
};

/** Every Session, unfiltered, most recent first: not the list on screen, whose Project / time filter may hide some. */
export const useAllSessions = (enabled = true) => useSessions(ALL_SESSIONS, enabled);

/** Session list for the current project / sort selection in the UI store. */
export function useSessionList() {
  const projectIds = useUi((s) => s.projectIds);
  const sort = useUi((s) => s.sessionSort);
  const descending = useUi((s) => s.sessionDescending);
  const drill = useUi((s) => s.statsDrill);
  const drillRange = useUi((s) => s.statsDrillRange);
  return useSessions({ projectIds, sort, descending, timeRange: drill ? drillRange : null, drill });
}

export const useSession = (sessionId: string | null) =>
  useQuery({
    queryKey: queryKeys.session(sessionId ?? ""),
    queryFn: () => api.getSession(sessionId!),
    enabled: sessionId != null,
  });

/**
 * A transcript left behind is dropped after this long, not the default 5 minutes: a long Session's runs to tens
 * of MB, and browsing the list with ↑ / ↓ would keep every one passed over.
 */
const TRANSCRIPT_GC_MS = 10_000;

export const useTranscript = (req: TranscriptRequest | null) =>
  useQuery({
    queryKey: req ? queryKeys.transcript(req) : ["transcript", null],
    queryFn: () => api.getTranscript(req!),
    enabled: req != null,
    placeholderData: keepPreviousData,
    gcTime: TRANSCRIPT_GC_MS,
  });

/** Transcript of the open session using the scope / branch / hidden state from the UI store. */
export function useCurrentTranscript() {
  const sessionId = useUi((s) => s.sessionId);
  const branchChoices = useUi((s) => s.branchChoices);
  const includeHidden = useUi((s) => s.showHidden);
  return useTranscript(sessionId ? { sessionId, scope: MAIN_SCOPE, branchChoices, includeHidden } : null);
}

export const useToolDetail = (req: ToolDetailRequest | null, enabled = true) =>
  useQuery({
    queryKey: req ? queryKeys.toolDetail(req) : ["toolDetail", null],
    queryFn: () => api.getToolDetail(req!),
    enabled: enabled && req != null,
    staleTime: Infinity,
  });

export const useImage = (req: ImageRequest | null) =>
  useQuery({
    queryKey: req ? queryKeys.image(req) : ["image", null],
    queryFn: () => api.getImage(req!),
    enabled: req != null,
    staleTime: Infinity,
  });

export const useFind = (req: FindRequest | null) =>
  useQuery({
    queryKey: req ? queryKeys.find(req) : ["find", null],
    queryFn: () => api.findInSession(req!),
    enabled: req != null && req.query !== "",
    placeholderData: keepPreviousData,
  });

export const useStats = (req: StatsRequest) =>
  useQuery({ queryKey: queryKeys.stats(req), queryFn: () => api.getStats(req), placeholderData: keepPreviousData });

export const useDiagnostics = () => useQuery({ queryKey: queryKeys.diagnostics, queryFn: () => api.getDiagnostics() });

/** Slash commands for a Connected Session started in `cwd`; asked again when stale (commands are files that change). */
/** Directories to complete `/add-dir <partial>` with; the last list stays while the next one loads. */
export const useDirCompletions = (cwd: string | null, partial: string, enabled: boolean) =>
  useQuery({
    queryKey: queryKeys.dirCompletions(cwd, partial),
    queryFn: () => api.completeDirs(cwd, partial),
    staleTime: 5_000,
    placeholderData: keepPreviousData,
    enabled,
  });

export const useSlashCommands = (cwd: string | null, enabled = true) =>
  useQuery({
    queryKey: queryKeys.slashCommands(cwd),
    queryFn: () => api.listSlashCommands(cwd),
    staleTime: 30_000,
    enabled,
  });
