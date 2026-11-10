import SwiftUI

private func copyIdMenu(_ id: String) -> some View {
    Button("复制 Session ID", systemImage: "doc.on.doc") { UIPasteboard.general.string = id }
}

private extension SessionSummary {
    var displayTitle: String { title.isEmpty ? "无标题" : title }
}

/// A card of 进行中: status word and time, the title, the Project.
struct SessionRunningCard: View {
    let session: SessionSummary
    let project: ProjectSummary?
    let now: Date
    @Environment(HostModel.self) private var model

    var body: some View {
        let status = session.live?.status ?? .unknown
        let c = model.connect.conn(for: session.id).flatMap { $0.isLive ? $0 : nil }
        NavigationLink(value: SessionRoute(id: session.id)) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Circle().fill(status.color).frame(width: 7, height: 7)
                    Text(status.label).foregroundStyle(status.wordColor)
                    Spacer(minLength: 0)
                    Text(time(c)).foregroundStyle(.secondary).monospacedDigit()
                }
                .font(.footnote)
                .lineLimit(1)
                Text(session.displayTitle)
                    .font(.system(size: 15))
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
                    .frame(maxHeight: .infinity, alignment: .topLeading)
                Text(project?.displayName ?? "")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .padding(12)
            .frame(maxWidth: .infinity, minHeight: 112, maxHeight: 112, alignment: .topLeading)
            .dynamicTypeSize(...DynamicTypeSize.xxxLarge)
            .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 16))
            .contentShape(RoundedRectangle(cornerRadius: 16))
        }
        .buttonStyle(.plain)
        .contextMenu { copyIdMenu(session.id) }
    }

    private func time(_ c: ConnState?) -> String {
        if let c, c.state == .busy, let s = c.progress?.elapsedS { return ConnectWords.elapsed(s) }
        return RelativeTime.format(session.lastActive, now: now)
    }
}

/// A row of 最近: the title and the time.
struct SessionRecentRow: View {
    let session: SessionSummary
    let now: Date

    var body: some View {
        NavigationLink(value: SessionRoute(id: session.id)) {
            HStack(spacing: 12) {
                Text(session.displayTitle).font(.system(size: 16)).lineLimit(1)
                Spacer(minLength: 0)
                Text(RelativeTime.format(session.lastActive, now: now)).font(.footnote).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 16)
            .frame(minHeight: 52)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu { copyIdMenu(session.id) }
    }
}

/// A row of the search results: the title, the Project, the time.
struct SessionSearchRow: View {
    let session: SessionSummary
    let project: ProjectSummary?
    let now: Date

    var body: some View {
        NavigationLink(value: SessionRoute(id: session.id)) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(session.displayTitle).font(.system(size: 16)).lineLimit(1)
                    Text(project?.displayName ?? "").font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
                Text(RelativeTime.format(session.lastActive, now: now)).font(.footnote).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 16)
            .frame(minHeight: 60)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu { copyIdMenu(session.id) }
    }
}

extension LiveStatus {
    /// Same words as the desktop sidebar.
    var label: String {
        switch self {
        case .busy: "工作中"
        case .waiting: "等待作答"
        case .idle: "空闲"
        case .unknown: "Live"
        }
    }

    var color: Color {
        switch self {
        case .busy: Palette.ok
        case .waiting: Palette.warn
        case .idle, .unknown: Palette.idle
        }
    }

    /// The status word's color.
    fileprivate var wordColor: Color {
        switch self {
        case .busy: Palette.ok
        case .waiting: Palette.warnText
        case .idle, .unknown: Color.secondary
        }
    }
}
