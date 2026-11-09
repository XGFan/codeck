import SwiftUI

/// The 「···」 menu in the navigation bar: while connected, Disconnect (the Session keeps running) and
/// 「关闭 Session」 (ends it with its Pane, after a confirmation; see `CloseFlow`); 「Fork」 for any indexed Session,
/// running or not (a new Session copying this one's conversation, in a new Pane).
struct ConnectMenu: View {
    let sessionId: String
    /// For Fork (the directory to start in); nil while loading or not indexed.
    let detail: SessionDetail?
    let title: String?
    @Environment(HostModel.self) private var model
    @State private var flow = CloseFlow()
    @State private var confirming = false
    @State private var closing = false
    @State private var confirmingFork = false

    var body: some View {
        let c = model.connect.conn(for: sessionId).flatMap { $0.isLive && !model.connect.isResuming(sessionId) ? $0 : nil }
        let fork = forkOffer
        if c != nil || fork != .hidden {
            Menu {
                if let c {
                    Section {
                        Button("Disconnect", systemImage: "cable.connector.slash") {
                            Task { await model.connect.disconnect(c) }
                        }
                        Button("关闭 Session", systemImage: "power", role: .destructive) {
                            flow = CloseFlow()
                            confirming = true
                        }
                        .disabled(closing || !(c.isBusy || c.state == .idle))
                    }
                }
                if fork != .hidden {
                    let forking = model.connect.isForking(sessionId)
                    Button {
                        confirmingFork = true
                    } label: {
                        Label(forking ? "Fork 中…" : "Fork", systemImage: "arrow.triangle.branch")
                        if case .disabled(let why) = fork { Text(why) }
                    }
                    .disabled(fork != .enabled || forking)
                }
            } label: {
                Label("更多", systemImage: "ellipsis.circle")
            }
            .confirmationDialog("关闭 Session？", isPresented: $confirming, titleVisibility: .visible) {
                if let c {
                    Button("关闭", role: .destructive) { Task { await close(c) } }
                }
                Button("取消", role: .cancel) {}
            } message: {
                Text(flow.message(busy: c?.isBusy ?? false, running: c?.runningTasks.count ?? 0))
            }
            .background {
                // A view of its own: two dialogs on one view do not both present.
                Color.clear.confirmationDialog("Fork 这个 Session？", isPresented: $confirmingFork, titleVisibility: .visible) {
                    Button("Fork") { startFork() }
                    Button("取消", role: .cancel) {}
                } message: {
                    Text("在新的 Pane 里起一个新 Session，复制这个 Session 的全部对话，原 Session 不受影响。")
                }
            }
            .sensoryFeedback(.warning, trigger: confirming) { _, new in new }
        }
    }

    private var forkOffer: StartOffer {
        guard let detail else { return .hidden }
        return StartOffer.fork(cwd: detail.cwd, cwdMissing: detail.cwdMissing ?? false, ccrpcError: model.connect.lsError)
    }

    private func startFork() {
        guard let cwd = detail?.cwd else { return }
        let store = model.connect
        let title = StartFlow.forkTitle(title ?? detail?.summary.title ?? "", taken: model.sessions.map(\.title))
        Task { await store.fork(sessionId: sessionId, cwd: cwd, title: title) }
    }

    private func close(_ c: ConnState) async {
        let store = model.connect
        closing = true
        defer { closing = false }
        if c.isBusy {
            let r = await store.command(c.conn, ["type": "abort"])
            if !r.success { return store.setError(sessionId, "关闭失败：\(r.errorText)") }
        }
        var cmd: [String: JSONValue] = ["type": "exit", "close_pane": true]
        if flow.stopTasks(running: c.runningTasks.count) { cmd["stop_tasks"] = true }
        let r = await store.command(c.conn, .object(cmd))
        switch flow.outcome(of: r) {
        case .done:
            // ccrpc stays on a closed Pane until told to go.
            await store.disconnect(c)
        case .confirmTasks: confirming = true
        case .failed(let text): store.setError(sessionId, text)
        }
    }
}
