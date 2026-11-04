import { getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-dialog";
import type { InMsg, LsEntry } from "@/features/drive/protocol";
import type { Api } from "./api";
import { commands, events } from "./bindings";

export function createRealApi(): Api {
  return {
    getAppInfo: commands.getAppInfo,
    setDataRoot: commands.setDataRoot,
    getIndexStatus: commands.getIndexStatus,
    rebuildIndex: commands.rebuildIndex,
    listProjects: commands.listProjects,
    listSessions: commands.listSessions,
    getSession: commands.getSession,
    getTranscript: commands.getTranscript,
    getToolDetail: commands.getToolDetail,
    getImage: commands.getImage,
    search: commands.search,
    resolveJump: commands.resolveJump,
    findInSession: commands.findInSession,
    revealSessionFile: commands.revealSessionFile,
    getStats: commands.getStats,
    getDiagnostics: commands.getDiagnostics,
    listSlashCommands: commands.listSlashCommands,
    onIndexStatus: (cb) => events.indexStatusEvent.listen((e) => cb(e.payload)),
    onSessionsChanged: (cb) => events.sessionsChangedEvent.listen((e) => cb(e.payload)),
    onLiveChanged: (cb) => events.liveChangedEvent.listen((e) => cb(e.payload)),
    onPricesChanged: (cb) => events.pricesChangedEvent.listen(() => cb()),
    copyText: writeText,
    driveInfo: commands.driveInfo,
    driveList: async () => JSON.parse(await commands.driveList()) as LsEntry[],
    driveConnect: commands.driveConnect,
    driveNew: commands.driveNew,
    driveSend: async (conn, msg) => void (await commands.driveSend(conn, JSON.stringify(msg))),
    driveDisconnect: async (conn) => void (await commands.driveDisconnect(conn)),
    driveGuiLog: async (conn, text) => void (await commands.driveGuiLog(conn, text)),
    driveLogDir: commands.driveLogDir,
    driveSaveImage: commands.driveSaveImage,
    pickDir: async () => {
      const r = await open({ directory: true, multiple: false });
      return typeof r === "string" ? r : null;
    },
    setBadge: (count) => getCurrentWindow().setBadgeCount(count > 0 ? count : undefined),
    onDriveLine: (cb) =>
      events.driveLineEvent.listen(({ payload }) => {
        let msg: InMsg;
        try {
          msg = JSON.parse(payload.line) as InMsg;
        } catch {
          return;
        }
        cb(payload.conn, msg);
      }),
    onDriveExit: (cb) => events.driveExitEvent.listen(({ payload }) => cb(payload.conn, payload.code, payload.stderr)),
  };
}
