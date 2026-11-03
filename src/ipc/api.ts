import type { InMsg, LsEntry } from "@/protocol";

export interface DoctorLine {
  level: "ok" | "warn" | "fail";
  text: string;
}

export interface AppInfo {
  /** The ccrpc executable used for every connection. */
  ccrpcPath: string;
  /** `ccrpc doctor`, run once at startup. */
  doctor: DoctorLine[];
  /** `CCRPC_ANALYZER_API_KEY` is set in the login shell's environment. */
  analyzer: boolean;
  logRoot: string;
}

/** The Tauri backend: one ccrpc child process per connection, JSONL both ways, logs. */
export interface Backend {
  appInfo(): Promise<AppInfo>;
  listSessions(): Promise<LsEntry[]>;
  /** `ccrpc attach --co-display`; returns the connection id. */
  connect(holderId: string): Promise<string>;
  /** `ccrpc new --co-display --cwd`; returns the connection id. */
  newSession(cwd: string): Promise<string>;
  send(conn: string, msg: Record<string, unknown>): Promise<void>;
  /** Closes ccrpc's stdin (the Session keeps running). */
  disconnect(conn: string): Promise<void>;
  /** A line in the connection's gui.log (or the global one). */
  guiLog(conn: string | null, text: string): Promise<void>;
  /** Stores a pasted image; returns its path on the Host. */
  saveImage(base64: string, ext: string): Promise<string>;
  pickDir(): Promise<string | null>;
  setBadge(count: number): Promise<void>;
  copyText(text: string): Promise<void>;
  onLine(cb: (conn: string, msg: InMsg) => void): () => void;
  /** ccrpc exited; `code` null when killed by a signal. */
  onExit(cb: (conn: string, code: number | null, stderr: string) => void): () => void;
}
