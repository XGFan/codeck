import SwiftUI

/// The native Connect bar pinned to the bottom of the Session page (`safeAreaInset`, so the Transcript scrolls
/// above it): 「Connect」 when the Session runs in clay, and once connected its state, the cards it waits on,
/// the input box and its controls; 「Resume」 for an ended Session not in clay.
struct ConnectBarSlot: View {
    let sessionId: String
    /// For Resume (the directory the Session started in); nil while loading.
    let detail: SessionDetail?
    @Environment(HostModel.self) private var model

    var body: some View {
        let store = model.connect
        let c = store.conn(for: sessionId)
        let resume = resumeOffer(c)
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
        } else if store.isResuming(sessionId) {
            bar {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Resume 中…").foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity)
            }
        } else if c != nil || store.lsEntry(for: sessionId) != nil || store.errors[sessionId] != nil || resume != .hidden {
            bar { NotConnectedBar(sessionId: sessionId, ended: c, resume: resume, cwd: detail?.cwd) }
        } else if let lsError = store.lsError, model.session(id: sessionId)?.live != nil {
            // A running Session may be in clay, but the Host has no `ccrpc ls` list (asked again on the next hello).
            bar {
                Text(lsError).font(.footnote).foregroundStyle(.secondary).frame(maxWidth: .infinity)
            }
        }
    }

    /// Resume: only for a Session neither in clay nor running elsewhere.
    private func resumeOffer(_ c: ConnState?) -> StartOffer {
        guard let detail, model.connect.lsEntry(for: sessionId) == nil, c?.isLive != true else { return .hidden }
        return StartOffer.resume(
            live: (model.session(id: sessionId) ?? detail.summary).live != nil,
            cwd: detail.cwd,
            cwdMissing: detail.cwdMissing ?? false,
            ccrpcError: model.connect.lsError
        )
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
    let resume: StartOffer
    let cwd: String?
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
            if resume != .hidden {
                Button {
                    guard let cwd else { return }
                    Task { await store.resume(sessionId: sessionId, cwd: cwd) }
                } label: {
                    Label("Resume", systemImage: "arrow.counterclockwise")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(resume != .enabled)
                if case .disabled(let why) = resume {
                    Text(why).font(.footnote).foregroundStyle(.secondary).frame(maxWidth: .infinity)
                }
            }
        }
    }
}

/// 「Session 已结束」, 「已 Disconnect」 (a client of this Host disconnected it: not a failure), or why ccrpc exited.
private struct EndedNotice: View {
    let c: ConnState

    var body: some View {
        let crashed = c.procExit != nil && !c.disconnected
        HStack(spacing: 10) {
            Image(systemName: crashed ? "exclamationmark.triangle" : c.disconnected ? "cable.connector.slash" : "stop.circle")
                .foregroundStyle(crashed ? .red : .secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(c.disconnected ? "已 Disconnect" : c.procExit.map(ConnectWords.procExit) ?? "Session 已结束")
                    .font(.subheadline.weight(.semibold))
                if crashed, let line = ConnectWords.lastLine(c.procExit?.stderr) {
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

