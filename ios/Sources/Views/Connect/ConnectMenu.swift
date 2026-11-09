import SwiftUI

/// The 「···」 menu in the navigation bar while connected: Disconnect (the Session keeps running) and
/// 「关闭 Session」 (ends it with its Pane, after a confirmation; see `CloseFlow`).
struct ConnectMenu: View {
    let sessionId: String
    @Environment(HostModel.self) private var model
    @State private var flow = CloseFlow()
    @State private var confirming = false
    @State private var closing = false

    var body: some View {
        if let c = model.connect.conn(for: sessionId), c.isLive {
            Menu {
                Button("Disconnect", systemImage: "cable.connector.slash") {
                    Task { await model.connect.disconnect(c) }
                }
                Button("关闭 Session", systemImage: "power", role: .destructive) {
                    flow = CloseFlow()
                    confirming = true
                }
                .disabled(closing || !(c.isBusy || c.state == .idle))
            } label: {
                Label("Connect", systemImage: "ellipsis.circle")
            }
            .confirmationDialog("关闭 Session？", isPresented: $confirming, titleVisibility: .visible) {
                Button("关闭", role: .destructive) { Task { await close(c) } }
                Button("取消", role: .cancel) {}
            } message: {
                Text(flow.message(busy: c.isBusy, running: c.runningTasks.count))
            }
            .sensoryFeedback(.warning, trigger: confirming) { _, new in new }
        }
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
        case .done: store.markEnding(c.conn)
        case .confirmTasks: confirming = true
        case .failed(let text): store.setError(sessionId, text)
        }
    }
}
