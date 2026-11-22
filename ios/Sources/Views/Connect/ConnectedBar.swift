import SwiftUI

/// A live Connect at the bottom of its Session page: the requests it waits on in a panel, otherwise the input card
/// (its controls in its bottom row) and, above it, queued messages and the last error; a notice shows briefly on top.
struct ConnectedBar: View {
    let c: ConnState
    let sessionId: String
    let panelMaxHeight: CGFloat
    @Environment(HostModel.self) private var model
    @State private var toast: String?
    @State private var seenNotice = Int.min
    // Up here so a draft outlives the requests that replace the input box.
    @State private var text = ""
    @State private var images: [ComposerImage] = []
    @State private var showingTasks = false
    @FocusState private var focused: Bool

    var body: some View {
        let store = model.connect
        let requests = store.requests(of: c)
        let error = store.errors[sessionId]
        VStack(spacing: 0) {
            if requests.isEmpty {
                VStack(spacing: 8) {
                    if store.rewinding[c.conn] != nil {
                        // ccrpc is finding the prompt in the terminal's Rewind picker.
                        Text("正在准备回退…")
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
                    if !c.queue.isEmpty { QueueChips(queue: c.queue, c: c, sessionId: sessionId) }
                    Composer(c: c, sessionId: sessionId, text: $text, images: $images, focused: $focused) {
                        ConnectControls(c: c, sessionId: sessionId, showingTasks: $showingTasks)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 8)
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
            show(text)
        }
        .onChange(of: store.notes[c.conn], initial: true) { _, note in
            guard let note else { return }
            store.notes[c.conn] = nil
            show(note)
        }
        .onChange(of: store.restoredPrompts[c.conn], initial: true) { _, prompt in
            // A rewind or a Fork put this prompt back into Claude Code's input box: into an empty one here, a
            // draft stays.
            guard let prompt else { return }
            store.restoredPrompts[c.conn] = nil
            if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, images.isEmpty { text = prompt }
        }
    }

    private func show(_ text: String) {
        withAnimation { toast = text }
        Task {
            try? await Task.sleep(for: .seconds(3))
            if toast == text { withAnimation { toast = nil } }
        }
    }
}

/// What waits in Claude Code's own queue; a message typed here can be taken back before it goes.
private struct QueueChips: View {
    let queue: [QueueItem]
    let c: ConnState
    let sessionId: String
    @Environment(HostModel.self) private var model

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                chip { Text("排队 \(queue.count) 条").foregroundStyle(.secondary) }
                ForEach(queue) { item in
                    chip {
                        HStack(spacing: 4) {
                            Text(item.label).lineLimit(1).frame(maxWidth: 220)
                            if item.editable {
                                Button { cancel(item) } label: {
                                    // Wide enough for a finger: the mark itself is small.
                                    Image(systemName: "xmark").font(.caption2)
                                        .frame(minWidth: 28, minHeight: 36)
                                        .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("撤回这条")
                            }
                        }
                    }
                }
            }
            .font(.footnote)
        }
        .scrollClipDisabled()
    }

    private func cancel(_ item: QueueItem) {
        let store = model.connect
        Task {
            let r = await store.command(c.conn, ["type": "cancel_queue", "queue_id": .string(item.id)])
            if !r.success { store.setError(sessionId, "撤回排队的消息失败：\(r.errorText)") }
        }
    }

    private func chip<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        content()
            .padding(.horizontal, 12)
            .frame(minHeight: 36)
            .glassCapsule()
    }
}
