import SwiftUI

struct SessionRow: View {
    let session: SessionSummary
    let project: ProjectSummary?
    let now: Date

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let live = session.live {
                    LiveDot(status: live.status)
                }
                Text(session.title.isEmpty ? "无标题" : session.title)
                    .font(.body)
                    .lineLimit(2)
            }
            HStack(spacing: 6) {
                if let live = session.live {
                    Text(live.status.label).foregroundStyle(live.status.color)
                    Text("·")
                }
                if let project {
                    Text(project.displayName).lineLimit(1)
                    Text("·")
                }
                Text(RelativeTime.format(session.lastActive, now: now))
                Text("·")
                Text("\(session.messageCount) 条消息")
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
            .lineLimit(1)
        }
        .padding(.vertical, 2)
    }
}

struct LiveDot: View {
    let status: LiveStatus

    var body: some View {
        Circle()
            .fill(status.color)
            .frame(width: 8, height: 8)
            .accessibilityLabel(status.label)
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
        case .busy: .green
        case .waiting: .orange
        case .idle: .gray
        case .unknown: .secondary
        }
    }
}
