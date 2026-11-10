import SwiftUI

/// The Session list's three sections.
struct SessionSections: Equatable {
    /// 等你处理: waiting on the user.
    var pending: [SessionSummary] = []
    /// 进行中: the other Live ones.
    var running: [SessionSummary] = []
    /// 最近: the rest.
    var recent: [SessionSummary] = []

    /// A Session is in exactly one section. `hasRequests` tells whether it has a live Connect with open
    /// requests (which may be ahead of the Live status). `pendingOrder` is the order 等你处理 was last shown in: its
    /// cards can be answered in place, so they keep their places and a newcomer goes last.
    init(_ sessions: [SessionSummary], pendingOrder: [String] = [], hasRequests: (String) -> Bool) {
        let byRecent: (SessionSummary, SessionSummary) -> Bool = { $0.lastActiveMs > $1.lastActiveMs }
        let waiting = sessions.filter { $0.live?.status == .waiting || hasRequests($0.id) }.sorted(by: byRecent)
        let rank = Dictionary(pendingOrder.enumerated().map { ($1, $0) }, uniquingKeysWith: { a, _ in a })
        pending = waiting.filter { rank[$0.id] != nil }.sorted { rank[$0.id]! < rank[$1.id]! }
            + waiting.filter { rank[$0.id] == nil }
        let rest = sessions.filter { s in !pending.contains { $0.id == s.id } }
        running = rest.filter { $0.live != nil }.sorted {
            let (a, b) = ($0.live!.status.urgency, $1.live!.status.urgency)
            return a != b ? a > b : byRecent($0, $1)
        }
        recent = rest.filter { $0.live == nil }.sorted(by: byRecent)
    }
}

/// A card of 等你处理. Only a permission that is the first request of a live Connect is answered here; everything else
/// opens the Session page.
struct PendingCard: View {
    /// The coordinate space of the list's content.
    nonisolated static let listSpace = "pending-list"

    let session: SessionSummary
    let project: ProjectSummary?
    @Environment(HostModel.self) private var model

    private var title: String { session.title.isEmpty ? "无标题" : session.title }
    private var projectName: String { project?.displayName ?? "" }

    var body: some View {
        let store = model.connect
        let c = store.conn(for: session.id).flatMap { $0.isLive ? $0 : nil }
        let requests = c.map { store.requests(of: $0) } ?? []
        Group {
            if let c, case .permission(let r)? = requests.first {
                PendingPermission(
                    sessionId: session.id, title: title, projectName: projectName, c: c, r: r,
                    more: requests.count - 1
                )
                // Another request is another card: nothing carries over, and it must be read before it can be answered.
                .id("\(c.conn)|\(r.requestId)")
            } else if let first = requests.first {
                link(subtitle: subtitle(first))
            } else if c == nil, store.lsEntry(for: session.id)?.plugin == true {
                connectable
            } else {
                link(subtitle: line(projectName, "等待作答"))
            }
        }
        .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 18))
        .overlay(RoundedRectangle(cornerRadius: 18).strokeBorder(Palette.warn.opacity(0.55), lineWidth: 1))
        .contextMenu {
            Button("复制 Session ID", systemImage: "doc.on.doc") { UIPasteboard.general.string = session.id }
        }
    }

    private func line(_ parts: String...) -> String {
        parts.filter { !$0.isEmpty }.joined(separator: " · ")
    }

    private func subtitle(_ request: ConnRequest) -> String {
        switch request {
        case .question(let r): line(projectName, "向你提问", "\(r.questions.count) 个问题")
        case .choice: line(projectName, "等待选择")
        default: line(projectName, "等待作答")
        }
    }

    private func titleBlock(_ subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.system(size: 17)).lineLimit(1)
            Text(subtitle).font(.footnote).foregroundStyle(.secondary).lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }

    private func link(subtitle: String) -> some View {
        NavigationLink(value: SessionRoute(id: session.id)) {
            HStack(spacing: 10) {
                titleBlock(subtitle)
                Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
        }
        .buttonStyle(.plain)
    }

    private var connectable: some View {
        let connecting = model.connect.connecting.contains(session.id)
        let error = model.connect.errors[session.id]
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 10) {
                NavigationLink(value: SessionRoute(id: session.id)) {
                    titleBlock(line(projectName, "等待作答", "未 Connect"))
                }
                .buttonStyle(.plain)
                Button {
                    Task { await model.connect.connect(sessionId: session.id) }
                } label: {
                    if connecting {
                        ProgressView()
                    } else {
                        Text("Connect")
                    }
                }
                .buttonStyle(PillButtonStyle(kind: .tinted, height: 44))
                .fixedSize()
                .disabled(connecting)
            }
            if let error { Text(error).font(.footnote).foregroundStyle(Palette.error) }
        }
        .padding(.vertical, 8)
        .padding(.leading, 14)
        .padding(.trailing, 8)
    }
}

/// A permission answered from the list. It can be answered only once it has been on screen, in this place, for a
/// moment, so a tap meant for what was there before cannot land on it; input too long to see here is approved on the
/// Session page.
private struct PendingPermission: View {
    let sessionId: String
    let title: String
    let projectName: String
    let c: ConnState
    let r: PermissionRequest
    let more: Int
    @Environment(HostModel.self) private var model
    @State private var armed = false
    @State private var overflowing = false
    /// Where it is in the list: scrolling does not change this, a card above it coming or going does.
    @State private var place: CGFloat?
    @State private var busy = false
    @State private var error: String?
    @State private var answered = 0

    private var route: SessionRoute { SessionRoute(id: sessionId) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            NavigationLink(value: route) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 17, weight: .semibold)).lineLimit(1)
                    Text([projectName, more > 0 ? "还有 \(more) 个" : ""].filter { !$0.isEmpty }.joined(separator: " · "))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text("允许 \(r.toolName)？").font(.subheadline.weight(.semibold))
                if let agent = r.agentId {
                    Text("Subagent \(agent.prefix(8))")
                        .font(.caption2)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Palette.neutral, in: RoundedRectangle(cornerRadius: 4))
                }
            }
            InputBlocksView(blocks: ConnectWords.inputBlocks(tool: r.toolName, r.input), maxHeight: 96) { overflowing = $0 }
            HStack(spacing: 8) {
                Button(Answers.permissionLabel(r, meaning: "deny", fallback: "拒绝")) { answer("deny") }
                    .buttonStyle(PillButtonStyle(kind: .neutral, height: 44))
                if overflowing {
                    NavigationLink("查看全部", value: route)
                        .buttonStyle(PillButtonStyle(kind: .tinted, height: 44))
                } else {
                    Button(Answers.permissionLabel(r, meaning: "allow", fallback: "允许")) { answer("allow") }
                        .buttonStyle(PillButtonStyle(height: 44))
                }
            }
            .disabled(busy || !armed)
            if let error { Text(error).font(.footnote).foregroundStyle(Palette.error) }
        }
        .padding(14)
        .sensoryFeedback(.success, trigger: answered)
        .onGeometryChange(for: CGFloat.self, of: { $0.frame(in: .named(PendingCard.listSpace)).minY.rounded() }) { place = $0 }
        .task(id: place) {
            armed = false
            try? await Task.sleep(for: .milliseconds(600))
            if !Task.isCancelled { armed = true }
        }
    }

    private func answer(_ decision: String) {
        let command: JSONValue = .object([
            "type": "answer_permission", "request_id": .string(r.requestId), "decision": .string(decision),
        ])
        Task {
            await sendAnswer(model.connect, c, r.requestId, command, busy: $busy, error: $error, answered: $answered)
        }
    }
}
