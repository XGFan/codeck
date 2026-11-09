import SwiftUI

/// The native Connect bar pinned to the bottom of the Session page (`safeAreaInset`, so the Transcript scrolls
/// above it): 「Connect」 when the Session runs in clay, and once connected its state, the cards it waits on,
/// the input box and its controls.
struct ConnectBarSlot: View {
    let sessionId: String
    @Environment(HostModel.self) private var model

    var body: some View {
        let store = model.connect
        let c = store.conn(for: sessionId)
        if let c, c.isLive {
            ConnectedBar(c: c, sessionId: sessionId)
                .id(c.conn)
        } else if store.connecting.contains(sessionId) {
            bar {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Connect 中…").foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity)
            }
        } else if c != nil || store.lsEntry(for: sessionId) != nil || store.errors[sessionId] != nil {
            bar { NotConnectedBar(sessionId: sessionId, ended: c) }
        } else if let lsError = store.lsError, model.session(id: sessionId)?.live != nil {
            // A running Session may be in clay, but the Host has no `ccrpc ls` list (asked again on the next hello).
            bar {
                Text(lsError).font(.footnote).foregroundStyle(.secondary).frame(maxWidth: .infinity)
            }
        }
        // Not in clay and never connected: nothing yet (Resume comes in a later phase).
    }

    private func bar<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        content()
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity)
            .background(.bar)
            .overlay(alignment: .top) { Divider() }
    }
}

/// Not connected: why a Connect ended, the last error, and 「Connect」 when the Holder can be connected.
private struct NotConnectedBar: View {
    let sessionId: String
    /// The Connect that ended, if any.
    let ended: ConnState?
    @Environment(HostModel.self) private var model

    var body: some View {
        let store = model.connect
        let entry = store.lsEntry(for: sessionId)
        VStack(alignment: .leading, spacing: 10) {
            if let ended {
                EndedNotice(c: ended)
            }
            if let error = store.errors[sessionId] {
                ErrorLine(text: error) { store.setError(sessionId, nil) }
            }
            if let entry, !store.holderConnected(entry.holderId) {
                if entry.plugin {
                    Button {
                        Task { await store.connect(sessionId: sessionId) }
                    } label: {
                        Label("Connect", systemImage: "link")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .sensoryFeedback(.impact, trigger: store.connecting.contains(sessionId))
                } else {
                    Text("ccrpc 插件未加载，在终端重启 claude 后可用")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity)
                }
            }
        }
    }
}

/// 「Session 已结束」, or why ccrpc exited.
private struct EndedNotice: View {
    let c: ConnState

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: c.procExit != nil ? "exclamationmark.triangle" : "stop.circle")
                .foregroundStyle(c.procExit != nil ? .red : .secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(c.procExit.map(ConnectWords.procExit) ?? "Session 已结束")
                    .font(.subheadline.weight(.semibold))
                if let line = ConnectWords.lastLine(c.procExit?.stderr) {
                    Text(line).font(.footnote).foregroundStyle(.secondary).lineLimit(2)
                }
            }
            Spacer(minLength: 0)
        }
    }
}

/// A failure to show near the input box; tap ✕ to dismiss.
struct ErrorLine: View {
    let text: String
    let onClose: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.circle.fill").foregroundStyle(.red)
            Text(text).font(.footnote).foregroundStyle(.red).frame(maxWidth: .infinity, alignment: .leading)
            Button("关闭", systemImage: "xmark", action: onClose)
                .labelStyle(.iconOnly)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }
}
