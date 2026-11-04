import type {
  AnalyzerInfo,
  AppInfo,
  Diagnostics,
  DriveInfo,
  FindRequest,
  FindResult,
  ImageData,
  ImageRequest,
  IndexStatus,
  IndexStatusEvent,
  JumpRequest,
  LaunchRule,
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
import type { InMsg, LsEntry } from "@/features/drive/protocol";

export type Unlisten = () => void;

/** One method per Tauri command, plus event subscriptions and the clipboard. Rejections are `AppError`. */
export interface Api {
  getAppInfo(): Promise<AppInfo>;
  setDataRoot(path: string | null): Promise<AppInfo>;
  getIndexStatus(): Promise<IndexStatus>;
  rebuildIndex(): Promise<null>;
  listProjects(): Promise<ProjectSummary[]>;
  listSessions(query: SessionQuery): Promise<SessionSummary[]>;
  getSession(sessionId: string): Promise<SessionDetail>;
  getTranscript(req: TranscriptRequest): Promise<Transcript>;
  getToolDetail(req: ToolDetailRequest): Promise<ToolDetail>;
  getImage(req: ImageRequest): Promise<ImageData>;
  search(req: SearchRequest): Promise<SearchResponse>;
  resolveJump(req: JumpRequest): Promise<JumpTarget>;
  findInSession(req: FindRequest): Promise<FindResult>;
  revealSessionFile(sessionId: string, agentId: string | null): Promise<null>;
  getStats(req: StatsRequest): Promise<Stats>;
  getDiagnostics(): Promise<Diagnostics>;
  /** Slash commands for completion while driving a Session started in `cwd`. */
  listSlashCommands(cwd: string | null): Promise<SlashCommand[]>;

  onIndexStatus(cb: (payload: IndexStatusEvent) => void): Promise<Unlisten>;
  onSessionsChanged(cb: (payload: SessionsChanged) => void): Promise<Unlisten>;
  onLiveChanged(cb: (payload: LiveChanged) => void): Promise<Unlisten>;
  onPricesChanged(cb: () => void): Promise<Unlisten>;

  copyText(text: string): Promise<void>;

  // Driving (ccrpc): one child process per connection, JSONL both ways, logs.
  /** ccrpc's path, `ccrpc doctor`, the analyzer, the log root; the first call reads the login shell's environment. */
  driveInfo(): Promise<DriveInfo>;
  /** Saves the analyzer's settings (blank = not set here); `apiKey: null` keeps the saved key, `""` removes it. */
  driveSetAnalyzer(apiKey: string | null, baseUrl: string | null, model: string | null): Promise<AnalyzerInfo>;
  /** Saves the launch rules (extra claude arguments by directory for 「新建 Session」); returns what was saved. */
  driveSetLaunchRules(rules: LaunchRule[]): Promise<LaunchRule[]>;
  /** `ccrpc ls --json`. */
  driveList(): Promise<LsEntry[]>;
  /** `ccrpc attach --co-display`; returns the connection id. */
  driveConnect(holderId: string): Promise<string>;
  /** `ccrpc new --co-display --cwd`; returns the connection id. */
  driveNew(cwd: string): Promise<string>;
  driveSend(conn: string, msg: object): Promise<void>;
  /** Closes ccrpc's stdin (the Session keeps running). */
  driveDisconnect(conn: string): Promise<void>;
  /** A line in the connection's gui.log (or the global one). */
  driveGuiLog(conn: string | null, text: string): Promise<void>;
  /** The connection's log directory; null once its ccrpc has exited. */
  driveLogDir(conn: string): Promise<string | null>;
  /** Stores a pasted image; returns its path. */
  driveSaveImage(base64: string, ext: string): Promise<string>;
  pickDir(): Promise<string | null>;
  /** The Dock badge; 0 clears it. */
  setBadge(count: number): Promise<void>;
  /** ccrpc's lines, parsed (lines that are not JSON are dropped). */
  onDriveLine(cb: (conn: string, msg: InMsg) => void): Promise<Unlisten>;
  /** ccrpc exited; `code` is null when killed by a signal. */
  onDriveExit(cb: (conn: string, code: number | null, stderr: string) => void): Promise<Unlisten>;
}
