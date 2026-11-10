import { getCurrentWindow } from "@tauri-apps/api/window";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-dialog";
import type { InMsg, LsEntry } from "@/features/connect/protocol";
import type { Api } from "./api";
import { commands, events } from "./bindings";

export function createRealApi(): Api {
  return {
    getAppInfo: commands.getAppInfo,
    setDataRoot: commands.setDataRoot,
    getIndexStatus: commands.getIndexStatus,
    rebuildIndex: commands.rebuildIndex,
    listProjects: commands.listProjects,
    getSidebarProjects: commands.getSidebarProjects,
    setSidebarProjects: commands.setSidebarProjects,
    listSessions: commands.listSessions,
    getSession: commands.getSession,
    getTranscript: commands.getTranscript,
    getToolDetail: commands.getToolDetail,
    getImage: commands.getImage,
    search: commands.search,
    resolveJump: commands.resolveJump,
    findInSession: commands.findInSession,
    revealSessionFile: commands.revealSessionFile,
    syncSession: commands.syncSession,
    getStats: commands.getStats,
    getDiagnostics: commands.getDiagnostics,
    listSlashCommands: commands.listSlashCommands,
    completePaths: commands.completePaths,
    onIndexStatus: (cb) => events.indexStatusEvent.listen((e) => cb(e.payload)),
    onSessionsChanged: (cb) => events.sessionsChangedEvent.listen((e) => cb(e.payload)),
    onLiveChanged: (cb) => events.liveChangedEvent.listen((e) => cb(e.payload)),
    onPricesChanged: (cb) => events.pricesChangedEvent.listen(() => cb()),
    copyText: writeText,
    setWindowTheme: (theme) => getCurrentWindow().setTheme(theme),
    ccrpcInfo: commands.ccrpcInfo,
    ccrpcSetAnalyzer: commands.ccrpcSetAnalyzer,
    ccrpcTestAnalyzer: commands.ccrpcTestAnalyzer,
    ccrpcAnalyzerRecords: commands.ccrpcAnalyzerRecords,
    ccrpcOpenAnalyzerRecords: commands.ccrpcOpenAnalyzerRecords,
    ccrpcSetLaunchRules: commands.ccrpcSetLaunchRules,
    ccrpcSetHideNewPanes: commands.ccrpcSetHideNewPanes,
    ccrpcSetPaneHidden: async (holderId, hidden) => void (await commands.ccrpcSetPaneHidden(holderId, hidden)),
    ccrpcWatchSessions: async (on) => void (await commands.ccrpcWatchSessions(on)),
    ccrpcConnect: commands.ccrpcConnect,
    ccrpcNew: commands.ccrpcNew,
    ccrpcResume: commands.ccrpcResume,
    ccrpcFork: commands.ccrpcFork,
    ccrpcSend: async (conn, msg) => void (await commands.ccrpcSend(conn, JSON.stringify(msg))),
    ccrpcDisconnect: async (conn) => void (await commands.ccrpcDisconnect(conn)),
    ccrpcGuiLog: async (conn, text) => void (await commands.ccrpcGuiLog(conn, text)),
    ccrpcLogDir: commands.ccrpcLogDir,
    ccrpcSaveImage: commands.ccrpcSaveImage,
    pickDir: async () => {
      const r = await open({ directory: true, multiple: false });
      return typeof r === "string" ? r : null;
    },
    setBadge: (count) => getCurrentWindow().setBadgeCount(count > 0 ? count : undefined),
    onCcrpcLine: (cb) =>
      events.ccrpcLineEvent.listen(({ payload }) => {
        let msg: InMsg;
        try {
          msg = JSON.parse(payload.line) as InMsg;
        } catch {
          return;
        }
        cb(payload.conn, msg);
      }),
    onCcrpcExit: (cb) => events.ccrpcExitEvent.listen(({ payload }) => cb(payload.conn, payload.code, payload.stderr)),
    onCcrpcLs: (cb) =>
      events.ccrpcLsEvent.listen(({ payload }) => {
        let ls: LsEntry[];
        try {
          ls = JSON.parse(payload.ls) as LsEntry[];
        } catch {
          return;
        }
        cb(ls);
      }),
    onCcrpcLsEnded: (cb) => events.ccrpcLsEndedEvent.listen(({ payload }) => cb(payload.message)),
    connectSnapshot: commands.connectSnapshot,
    onCcrpcRemoteConn: (cb) => events.ccrpcRemoteConnEvent.listen(({ payload }) => cb(payload.conn, payload.holderId)),
    onCcrpcClosing: (cb) => events.ccrpcClosingEvent.listen(({ payload }) => cb(payload.conn)),
    lanStatus: commands.lanStatus,
    lanSetEnabled: commands.lanSetEnabled,
  };
}
