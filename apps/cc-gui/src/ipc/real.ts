import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-dialog";
import type { InMsg, LsEntry } from "@/protocol";
import type { AppInfo, Backend } from "./api";

function subscribe<T>(name: string, cb: (p: T) => void): () => void {
  let un: (() => void) | null = null;
  let gone = false;
  void listen<T>(name, (e) => cb(e.payload)).then((f) => {
    if (gone) f();
    else un = f;
  });
  return () => {
    gone = true;
    un?.();
  };
}

export const realBackend: Backend = {
  appInfo: () => invoke<AppInfo>("app_info"),
  listSessions: () => invoke<LsEntry[]>("list_sessions"),
  connect: (holderId) => invoke<string>("connect", { holderId }),
  newSession: (cwd) => invoke<string>("new_session", { cwd }),
  send: (conn, msg) => invoke("send", { conn, line: JSON.stringify(msg) }),
  disconnect: (conn) => invoke("disconnect", { conn }),
  guiLog: (conn, text) => invoke("gui_log", { conn, text }),
  logDir: (conn) => invoke<string | null>("log_dir", { conn }),
  saveImage: (base64, ext) => invoke<string>("save_image", { base64, ext }),
  pickDir: async () => {
    const r = await open({ directory: true, multiple: false });
    return typeof r === "string" ? r : null;
  },
  setBadge: async (count) => {
    await getCurrentWindow().setBadgeCount(count > 0 ? count : undefined);
  },
  copyText: (text) => writeText(text),
  onLine: (cb) =>
    subscribe<{ conn: string; line: string }>("ccrpc-line", ({ conn, line }) => {
      let msg: InMsg;
      try {
        msg = JSON.parse(line) as InMsg;
      } catch {
        return;
      }
      cb(conn, msg);
    }),
  onExit: (cb) =>
    subscribe<{ conn: string; code: number | null; stderr: string }>("ccrpc-exit", (p) => cb(p.conn, p.code, p.stderr)),
};
