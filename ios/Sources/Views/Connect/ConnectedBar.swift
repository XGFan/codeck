import SwiftUI

/// A live Connect at the bottom of its Session page: the requests it waits on in a panel, otherwise the input box
/// with its controls and, above them, queued messages and the last error; a notice shows briefly on top.
struct ConnectedBar: View {
    let c: ConnState
    let sessionId: String
    /// The Transcript is at its end (the control row is put away when it is not).
    let atBottom: Bool
    let panelMaxHeight: CGFloat
    @Environment(HostModel.self) private var model
    @State private var toast: String?
    @State private var seenNotice = Int.min
    // Up here so a draft outlives the requests that replace the input box.
    @State private var text = ""
    @State private var images: [ComposerImage] = []
    @State private var showingTasks = false
    @FocusState private var focused: Bool

    /// The control row: put away while the Transcript is read further up, unless something needs it; a waiting
    /// Session has none of its controls to offer.
    static func showsControls(state: ConnState.Phase?, atBottom: Bool, focused: Bool, hasError: Bool, hasQueue: Bool) -> Bool {
        state != .waiting && (atBottom || focused || hasError || hasQueue)
    }

    var body: some View {
        let store = model.connect
        let requests = store.requests(of: c)
        let error = store.errors[sessionId]
        VStack(spacing: 0) {
            if requests.isEmpty {
                VStack(spacing: 8) {
                    if c.state == .waiting, c.waiting?.kind != "question" {
                        // Waiting with nothing to show yet: the analyzer may be looking at the screen.
                        Text("正在识别界面…")
                            .font(.footnote)
                            .foregroundStyle(Palette.warnText)
                            .padding(.horizontal, 14)
                            .frame(minHeight: 36)
                            .glassCapsule()
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    if let error {
                        ErrorLine(text: error) { store.setError(sessionId, nil) }
                    }
                    if !c.queue.isEmpty { QueueChips(queue: c.queue) }
                    if Self.showsControls(
                        state: c.state, atBottom: atBottom, focused: focused,
                        hasError: error != nil, hasQueue: !c.queue.isEmpty
                    ) {
                        ConnectControls(c: c, sessionId: sessionId, showingTasks: $showingTasks)
                            .transition(.opacity.combined(with: .move(edge: .bottom)))
                    }
                    Composer(c: c, sessionId: sessionId, text: $text, images: $images, focused: $focused)
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 8)
                .animation(.snappy, value: atBottom)
            } else {
                RequestPanel(
                    requests: requests,
                    c: c,
                    error: error,
                    onCloseError: { store.setError(sessionId, nil) },
                    maxHeight: panelMaxHeight
                )
                .transition(.move(edge: .bottom))
            }
        }
        .animation(.snappy, value: requests.isEmpty)
        .overlay(alignment: .top) {
            // Above the rest: the container's bottom sits 8 pt over its top.
            ZStack {
                if let toast {
                    Label(toast, systemImage: "info.circle")
                        .font(.footnote)
                        .lineLimit(2)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .glassCapsule()
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .alignmentGuide(.top) { $0[.bottom] + 8 }
        }
        .sheet(isPresented: $showingTasks) {
            TasksSheet(sessionId: sessionId, conn: c.conn)
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

/// What waits in Claude Code's own queue.
private struct QueueChips: View {
    let queue: [String]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                chip { Text("排队 \(queue.count) 条").foregroundStyle(.secondary) }
                ForEach(Array(queue.enumerated()), id: \.offset) { _, text in
                    chip { Text(text).lineLimit(1).frame(maxWidth: 220) }
                }
            }
            .font(.footnote)
        }
        .scrollClipDisabled()
    }

    private func chip<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        content()
            .padding(.horizontal, 12)
            .frame(minHeight: 36)
            .glassCapsule()
    }
}
