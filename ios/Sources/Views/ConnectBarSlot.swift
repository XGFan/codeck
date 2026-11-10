import SwiftUI

/// What floats at the bottom of the Session page: 「Connect」 when the Session runs in clay, and once connected
/// the requests it waits on or the input box and its controls; 「Resume」 for an ended Session not in clay.
struct ConnectBarSlot: View {
    let sessionId: String
    /// For Resume (the directory the Session started in); nil while loading.
    let detail: SessionDetail?
    let atBottom: Bool
    let panelMaxHeight: CGFloat
    @Environment(HostModel.self) private var model

    var body: some View {
        // A VStack that is there even with nothing in it: the page measures it for the Transcript's bottom inset.
        VStack(spacing: 0) {
            content
            Color.clear.frame(height: 0)
        }
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder
    private var content: some View {
        let store = model.connect
        let c = store.conn(for: sessionId)
        let resume = resumeOffer(c)
        // A Resume shows its connection only for what it asks before starting (the folder trust dialog).
        if let c, c.isLive, !store.isResuming(sessionId) || !store.requests(of: c).isEmpty {
            ConnectedBar(c: c, sessionId: sessionId, atBottom: atBottom, panelMaxHeight: panelMaxHeight)
                .id(c.conn)
        } else if store.connecting.contains(sessionId) {
            inProgress("Connect 中…")
        } else if store.isResuming(sessionId) {
            inProgress("Resume 中…")
        } else if c != nil || store.lsEntry(for: sessionId) != nil || store.errors[sessionId] != nil || resume != .hidden {
            NotConnectedBar(sessionId: sessionId, ended: c, resume: resume, cwd: detail?.cwd)
                .padding(.horizontal, 8)
                .padding(.bottom, 8)
        } else if let lsError = store.lsError, model.session(id: sessionId)?.live != nil {
            // A running Session may be in clay, but the Host has no `ccrpc ls` list (asked again on the next hello).
            GlassNote(text: lsError)
                .padding(.horizontal, 8)
                .padding(.bottom, 8)
        }
    }

    private func inProgress(_ word: String) -> some View {
        HStack(spacing: 8) {
            ProgressView()
            Text(word).font(.system(size: 16)).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .frame(height: 52)
        .glassCapsule()
        .padding(.horizontal, 8)
        .padding(.bottom, 8)
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
        VStack(spacing: 8) {
            if let ended {
                EndedNotice(c: ended)
            }
            if let error = store.errors[sessionId] {
                ErrorLine(text: error) { store.setError(sessionId, nil) }
            }
            if let entry, !store.holderConnected(entry.holderId) {
                if entry.plugin {
                    Button("Connect") {
                        Task { await store.connect(sessionId: sessionId) }
                    }
                    .buttonStyle(PillButtonStyle())
                    .sensoryFeedback(.impact, trigger: store.connecting.contains(sessionId))
                } else {
                    GlassNote(text: "ccrpc 插件未加载，在终端重启 claude 后可用")
                }
            }
            if resume != .hidden {
                if case .disabled(let why) = resume { GlassNote(text: why) }
                Button("Resume") {
                    guard let cwd else { return }
                    Task { await store.resume(sessionId: sessionId, cwd: cwd) }
                }
                .buttonStyle(PillButtonStyle())
                .disabled(resume != .enabled)
            }
        }
    }
}

/// A sentence on glass, where a Connect or Resume cannot be offered.
private struct GlassNote: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.footnote)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity)
            .glass(in: RoundedRectangle(cornerRadius: 22, style: .continuous))
    }
}

/// 「Session 已结束」, 「已 Disconnect」 (a client of this Host disconnected it: not a failure), or why ccrpc exited.
private struct EndedNotice: View {
    let c: ConnState

    var body: some View {
        let crashed = c.procExit != nil && !c.disconnected
        HStack(spacing: 10) {
            Image(systemName: crashed ? "exclamationmark.triangle" : c.disconnected ? "cable.connector.slash" : "stop.circle")
                .foregroundStyle(crashed ? Palette.error : .secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(c.disconnected ? "已 Disconnect" : c.procExit.map(ConnectWords.procExit) ?? "Session 已结束")
                    .font(.system(size: 15, weight: .semibold))
                if crashed, let line = ConnectWords.lastLine(c.procExit?.stderr) {
                    Text(line).font(.footnote).foregroundStyle(.secondary).lineLimit(2)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .glass(in: RoundedRectangle(cornerRadius: 22, style: .continuous))
    }
}

/// A failure to show near the input box; tap ✕ to dismiss.
struct ErrorLine: View {
    let text: String
    /// Floating on its own over the Transcript, not on a panel.
    var onGlass = true
    let onClose: () -> Void

    var body: some View {
        let line = HStack(spacing: 4) {
            Text(text)
                .font(.subheadline)
                .foregroundStyle(Palette.error)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("关闭", systemImage: "xmark", action: onClose)
                .labelStyle(.iconOnly)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.secondary)
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .padding(.leading, 16)
        if onGlass {
            line.glass(in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        } else {
            line
        }
    }
}
