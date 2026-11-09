import SwiftUI

/// A live Connect: its state, the cards it waits on, queued messages, the last error, the input box and its
/// controls; a notice shows briefly above the bar.
struct ConnectedBar: View {
    let c: ConnState
    let sessionId: String
    @Environment(HostModel.self) private var model
    @State private var toast: String?
    @State private var seenNotice = Int.min
    @State private var cardsHeight: CGFloat = 0

    var body: some View {
        let store = model.connect
        let requests = store.requests(of: c)
        VStack(spacing: 8) {
            StatusRow(c: c)
            if !requests.isEmpty {
                ScrollView {
                    VStack(spacing: 8) {
                        ForEach(requests) { RequestCard(request: $0, c: c) }
                    }
                    .onGeometryChange(for: CGFloat.self, of: \.size.height) { cardsHeight = $0 }
                }
                .frame(height: min(cardsHeight, 380))
                .scrollBounceBehavior(.basedOnSize)
            } else if c.state == .waiting, c.waiting?.kind != "question" {
                // Waiting with nothing to show yet: the analyzer may be looking at the screen.
                Label("正在识别界面…", systemImage: "text.viewfinder")
                    .font(.footnote)
                    .foregroundStyle(.orange)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            if !c.queue.isEmpty { QueueChips(queue: c.queue) }
            if let error = store.errors[sessionId] {
                ErrorLine(text: error) { store.setError(sessionId, nil) }
            }
            Composer(c: c, sessionId: sessionId)
            ConnectControls(c: c, sessionId: sessionId)
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .overlay(alignment: .top) {
            // Above the bar: the container's bottom sits 8 pt over the bar's top.
            ZStack {
                if let toast {
                Label(toast, systemImage: "info.circle")
                    .font(.footnote)
                    .lineLimit(2)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(.regularMaterial, in: Capsule())
                    .shadow(color: .black.opacity(0.08), radius: 6, y: 2)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .alignmentGuide(.top) { $0[.bottom] + 8 }
        }
        .sensoryFeedback(trigger: requests.count) { old, new in new > old ? .warning : nil }
        .onAppear { seenNotice = c.notices.last?.id ?? Int.min }
        .onChange(of: c.notices.last?.id) { _, id in
            guard let id, id > seenNotice, let text = c.notices.last?.text else { return }
            seenNotice = id
            withAnimation { toast = text }
            Task {
                try? await Task.sleep(for: .seconds(3))
                if toast == text { withAnimation { toast = nil } }
            }
        }
    }
}

/// The state word, what Claude Code is doing and for how long.
private struct StatusRow: View {
    let c: ConnState

    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(ConnectWords.state(c.state)).fontWeight(.medium)
            if c.state == .busy {
                if let text = c.progress?.text ?? c.progress?.verb {
                    Text(text).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
                }
                Spacer(minLength: 4)
                if let s = c.progress?.elapsedS {
                    Text(ConnectWords.elapsed(s)).foregroundStyle(.secondary).monospacedDigit()
                }
            } else {
                Spacer(minLength: 0)
            }
        }
        .font(.footnote)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var color: Color {
        switch c.state {
        case .busy: .green
        case .waiting: .orange
        case .idle: .secondary
        default: .gray.opacity(0.5)
        }
    }
}

/// What waits in Claude Code's own queue.
private struct QueueChips: View {
    let queue: [String]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                Label("排队 \(queue.count) 条", systemImage: "clock")
                    .foregroundStyle(.secondary)
                ForEach(Array(queue.enumerated()), id: \.offset) { _, text in
                    Text(text)
                        .lineLimit(1)
                        .frame(maxWidth: 220)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
                }
            }
            .font(.footnote)
        }
    }
}
