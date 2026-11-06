import type {
  AnalyzerInfo,
  AnalyzerRecords,
  AnalyzerTest,
  AppInfo,
  Diagnostics,
  CcrpcInfo,
  FindRequest,
  FindResult,
  ImageData,
  ImageRequest,
  IndexStatus,
  IndexStatusEvent,
  JumpRequest,
  LaunchRule,
  SidebarProject,
  JumpTarget,
  LiveChanged,
  ProjectSummary,
  SearchRequest,
  SearchResponse,
  SessionDetail,
  SessionQuery,
  SessionsChanged,
  SessionSummary,
  SlashCommand,
  Stats,
  StatsRequest,
  ToolDetail,
  ToolDetailRequest,
  Transcript,
  TranscriptRequest,
} from "./bindings";
import type { InMsg, LsEntry } from "@/features/connect/protocol";

export type Unlisten = () => void;

/** One method per Tauri command, plus event subscriptions and the clipboard. Rejections are `AppError`. */
export interface Api {
  getAppInfo(): Promise<AppInfo>;
  setDataRoot(path: string | null): Promise<AppInfo>;
  getIndexStatus(): Promise<IndexStatus>;
  rebuildIndex(): Promise<null>;
  listProjects(): Promise<ProjectSummary[]>;
  /** The Projects in the sidebar, in the order added; null before any were added. */
  getSidebarProjects(): Promise<SidebarProject[] | null>;
  /** Saves the Projects in the sidebar; returns what was saved (paths trimmed, repeats dropped). */
  setSidebarProjects(projects: SidebarProject[]): Promise<SidebarProject[]>;
  listSessions(query: SessionQuery): Promise<SessionSummary[]>;
  getSession(sessionId: string): Promise<SessionDetail>;
  getTranscript(req: TranscriptRequest): Promise<Transcript>;
  getToolDetail(req: ToolDetailRequest): Promise<ToolDetail>;
  getImage(req: ImageRequest): Promise<ImageData>;
  search(req: SearchRequest): Promise<SearchResponse>;
  resolveJump(req: JumpRequest): Promise<JumpTarget>;
  findInSession(req: FindRequest): Promise<FindResult>;
  revealSessionFile(sessionId: string, agentId: string | null): Promise<null>;
  /** Syncs the Session's file now (a prompt was just sent), without the watcher's debounce. */
  syncSession(sessionId: string): Promise<null>;
  getStats(req: StatsRequest): Promise<Stats>;
  getDiagnostics(): Promise<Diagnostics>;
  /** Slash commands for completion while connected to a Session started in `cwd`. */
  listSlashCommands(cwd: string | null): Promise<SlashCommand[]>;
  /** Paths to complete `partial` with in a Session started in `cwd`: directories (`/add-dir`), or `files` too (`@`). */
  completePaths(cwd: string | null, partial: string, files: boolean): Promise<string[]>;

  onIndexStatus(cb: (payload: IndexStatusEvent) => void): Promise<Unlisten>;
  onSessionsChanged(cb: (payload: SessionsChanged) => void): Promise<Unlisten>;
  onLiveChanged(cb: (payload: LiveChanged) => void): Promise<Unlisten>;
  onPricesChanged(cb: () => void): Promise<Unlisten>;

  copyText(text: string): Promise<void>;
  /** The window's own theme (title bar, menus, dialogs); null follows the system. */
  setWindowTheme(theme: "light" | "dark" | null): Promise<void>;

  // ccrpc: one child process per connection, JSONL both ways, logs.
  /** ccrpc's path, `ccrpc doctor`, the analyzer, the log root; the first call reads the login shell's environment. */
  ccrpcInfo(): Promise<CcrpcInfo>;
  /** Saves the analyzer's settings (blank = not set here); `apiKey: null` keeps the saved key, `""` removes it. */
  ccrpcSetAnalyzer(apiKey: string | null, baseUrl: string | null, model: string | null): Promise<AnalyzerInfo>;
  /** `ccrpc analyzer test` with these values, unsaved (`apiKey: null` = the saved or login shell's key); rejects with the reason. */
  ccrpcTestAnalyzer(apiKey: string | null, baseUrl: string | null, model: string | null): Promise<AnalyzerTest>;
  /** `ccrpc analyzer records`: the analysis records' directory and each record, newest first. */
  ccrpcAnalyzerRecords(): Promise<AnalyzerRecords>;
  /** Opens the analysis records' directory in Finder. */
  ccrpcOpenAnalyzerRecords(): Promise<null>;
  /** Saves the launch rules (extra claude arguments by directory for 「新建 Session」); returns what was saved. */
  ccrpcSetLaunchRules(rules: LaunchRule[]): Promise<LaunchRule[]>;
  /** Saves 「新开的 Pane」: whether Sessions started from now on (new and Resume) open hidden on the ToH Mac Client. */
  ccrpcSetHideNewPanes(hide: boolean): Promise<boolean>;
  /** `ccrpc hide|show <holderId>`: hides a clay Session's Pane on the ToH Mac Client, or shows it again. */
  ccrpcSetPaneHidden(holderId: string, hidden: boolean): Promise<void>;
  /** Starts (`on`) or stops `ccrpc ls --json --watch`; starting again restarts it, its first list comes at once. */
  ccrpcWatchSessions(on: boolean): Promise<void>;
  /** `ccrpc attach`; returns the connection id. */
  ccrpcConnect(holderId: string): Promise<string>;
  /** `ccrpc new --cwd`; returns the connection id. */
  ccrpcNew(cwd: string): Promise<string>;
  /** Resume: `ccrpc new --cwd <cwd> -- --resume <sessionId>` in a new Pane; returns the connection id. */
  ccrpcResume(sessionId: string, cwd: string): Promise<string>;
  /**
   * Fork: `ccrpc new --cwd <cwd> --fork <sessionId> [--fork-at <at>] [--fork-restore <restore>] --fork-title <title>` in a
   * new Pane; returns the connection id (the result comes as its `fork` event).
   */
  ccrpcFork(
    sessionId: string,
    cwd: string,
    at: string | null,
    restore: "conversation" | "both" | null,
    title: string,
  ): Promise<string>;
  ccrpcSend(conn: string, msg: object): Promise<void>;
  /** Closes ccrpc's stdin (the Session keeps running). */
  ccrpcDisconnect(conn: string): Promise<void>;
  /** A line in the connection's gui.log (or the global one). */
  ccrpcGuiLog(conn: string | null, text: string): Promise<void>;
  /** The connection's log directory; null once its ccrpc has exited. */
  ccrpcLogDir(conn: string): Promise<string | null>;
  /** Stores a pasted image; returns its path. */
  ccrpcSaveImage(base64: string, ext: string): Promise<string>;
  pickDir(): Promise<string | null>;
  /** The Dock badge; 0 clears it. */
  setBadge(count: number): Promise<void>;
  /** ccrpc's lines, parsed (lines that are not JSON are dropped). */
  onCcrpcLine(cb: (conn: string, msg: InMsg) => void): Promise<Unlisten>;
  /** ccrpc exited; `code` is null when killed by a signal. */
  onCcrpcExit(cb: (conn: string, code: number | null, stderr: string) => void): Promise<Unlisten>;
  /** `ccrpc ls --json --watch` printed the list (again). */
  onCcrpcLs(cb: (ls: LsEntry[]) => void): Promise<Unlisten>;
  /** `ccrpc ls --json --watch` ended by itself. */
  onCcrpcLsEnded(cb: (message: string) => void): Promise<Unlisten>;
}
