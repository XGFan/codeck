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
  LanDevicesEvent,
  LanStatus,
  LiveChanged,
  NotifyInfo,
  NotifySettings,
  PairedDevice,
  PairingOffer,
  PlanUsage,
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
import type { InMsg, LsEntry } from "@/ipc/protocol/ccrpc";
import type { HaaWorker } from "@/ipc/protocol/haa";

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
  /** `codeck://` links opened in the system, including the one the app was launched with. */
  onOpenUrl(cb: (urls: string[]) => void): Promise<Unlisten>;

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
  /** 「缓存保活」: minutes an idle Live Session's prompt cache goes unused before the Host refreshes it (0 = off). */
  keepAliveMinutes(): Promise<number>;
  /** Saves 「缓存保活」; returns what was saved. */
  setKeepAliveMinutes(minutes: number): Promise<number>;
  /** The Host starts (`active`) or ends refreshing a Session's prompt cache; `error` is why it failed. */
  onKeepAlive(cb: (e: { sessionId: string; active: boolean; error: string | null }) => void): Promise<Unlisten>;
  /** The Dock badge; 0 clears it. */
  setBadge(count: number): Promise<void>;
  /** A system notification (the Host sends them) was clicked: its `url` (a `codeck://` link). */
  onNotificationClick(cb: (url: string) => void): Promise<Unlisten>;
  /** Tells the Host which Session this window looks at (null: none): what counts as seen for the notifications. */
  setViewing(sessionId: string | null): Promise<void>;
  /** The 「已完成」 Sessions now (the Host decides them; later changes come to `onAttention`). */
  attentionDone(): Promise<string[]>;
  /** The 「已完成」 Sessions changed. */
  onAttention(cb: (done: string[]) => void): Promise<Unlisten>;
  /** 「设置 › 通用」 的通知 and how the last push to the iPhone went. */
  notifySettings(): Promise<NotifyInfo>;
  /** Saves 「设置 › 通用」 的通知; returns what was saved. */
  setNotifySettings(settings: NotifySettings): Promise<NotifyInfo>;
  /** Pushes a test notification to Bark with these values (not saved). */
  testBark(server: string, deviceKey: string): Promise<void>;
  /** ccrpc's lines, parsed (lines that are not JSON are dropped). */
  onCcrpcLine(cb: (conn: string, msg: InMsg) => void): Promise<Unlisten>;
  /** ccrpc exited; `code` is null when killed by a signal. */
  onCcrpcExit(cb: (conn: string, code: number | null, stderr: string) => void): Promise<Unlisten>;
  /** `ccrpc ls --json --watch` printed the list (again). */
  onCcrpcLs(cb: (ls: LsEntry[]) => void): Promise<Unlisten>;
  /** `ccrpc ls --json --watch` ended by itself. */
  onCcrpcLsEnded(cb: (message: string) => void): Promise<Unlisten>;
  /** The Host's connections as it folds them (a JSON array of `HostConn`), for a window that lost its own state. */
  connectSnapshot(): Promise<string>;
  /** A client on the LAN started or shared this connection. */
  onCcrpcRemoteConn(cb: (conn: string, holderId: string | null) => void): Promise<Unlisten>;
  /** This connection is being closed on purpose by a client on the LAN (or replaced), before its exit. */
  onCcrpcClosing(cb: (conn: string) => void): Promise<Unlisten>;

  /** 「局域网访问」: whether this Mac is a Host on the LAN, and on which addresses. */
  lanStatus(): Promise<LanStatus>;
  /** Turns 「局域网访问」 on or off at once and saves it. */
  lanSetEnabled(on: boolean): Promise<LanStatus>;
  /** Starts a 5-minute LAN pairing offer (QR code SVG and `codeck://pair` link), replacing any previous one. */
  lanPairStart(): Promise<PairingOffer>;
  /** Cancels the active LAN pairing offer if its id matches `id`. */
  lanPairCancel(id: string): Promise<void>;
  /** Paired LAN devices (readable even when LAN access is off). */
  lanDevices(): Promise<PairedDevice[]>;
  /** Removes a paired LAN device and closes its active connections; missing ids are ignored. */
  lanRemoveDevice(id: string): Promise<void>;
  /** The paired LAN device list changed (a device paired or was removed). */
  onLanDevices(cb: (e: LanDevicesEvent) => void): Promise<Unlisten>;

  /**
   * Starts (`on = true`) a `haa ls --all --json --watch` tagged with `id` (replacing any existing watch),
   * or stops the existing watch (`on = false`; `id` is unused). Desktop only.
   */
  haaWatch(on: boolean, id: number): Promise<void>;
  /** `haa ls --all --json --watch` (started with `id`) printed the Worker list (again). */
  onHaaWorkers(cb: (id: number, workers: HaaWorker[]) => void): Promise<Unlisten>;
  /** `haa ls --all --json --watch` (started with `id`) ended by itself. */
  onHaaWatchEnded(cb: (id: number, message: string) => void): Promise<Unlisten>;

  /** Reads the logged-in account's subscription usage via headless `claude` (`get_usage`); null when unavailable. */
  getPlanUsage(refresh: boolean): Promise<PlanUsage | null>;
}
