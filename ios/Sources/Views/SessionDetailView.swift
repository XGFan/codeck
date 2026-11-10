import SwiftUI

/// One Session: the Host's Transcript page across the whole screen, with the header and the Connect controls floating
/// over it, and native loading / error states.
struct SessionDetailView: View {
    @Environment(HostModel.self) private var model
    @State private var sessionId: String
    @State private var detail: SessionDetail?
    @State private var phase = Phase.loading
    @State private var reloadToken = 0
    @State private var copied = 0
    @State private var showingCopied = false
    @State private var showingInfo = false
    @State private var atBottom = true
    /// In screen coordinates, as the full-screen Transcript sees them.
    @State private var screen = CGRect.zero
    @State private var visibleBottom: CGFloat = 0
    @State private var headerBottom: CGFloat = 0
    @State private var slotTop: CGFloat?

    enum Phase: Equatable {
        case loading, ready
        case failed(String)
    }

    init(sessionId: String) {
        _sessionId = State(initialValue: sessionId)
    }

    private var summary: SessionSummary? { model.session(id: sessionId) ?? detail?.summary }

    /// Started from here and not indexed yet: no transcript before its first prompt.
    private var unindexed: Bool { summary == nil && model.connect.conn(for: sessionId) != nil }

    private var title: String { summary?.title ?? (unindexed ? "新 Session" : "Session") }

    /// Connected: what the Connect is doing. Otherwise, while the Session runs somewhere, its Live status.
    private var headerStatus: HeaderStatus? {
        if let c = model.connect.conn(for: sessionId), c.isLive, !model.connect.isResuming(sessionId) {
            return .connected(c)
        }
        return summary?.live.map { HeaderStatus(text: $0.status.label, dot: $0.status.color) }
    }

    private var hasRequests: Bool {
        guard let c = model.connect.conn(for: sessionId), c.isLive else { return false }
        return !model.connect.requests(of: c).isEmpty
    }

    /// How much of the screen, keyboard included, the bottom floats cover.
    private var bottomInset: CGFloat { slotTop.map { max(0, screen.maxY - $0) } ?? 0 }

    var body: some View {
        ZStack {
            Palette.transcriptGround
                // Inside `ignoresSafeArea`: outside it this would be the area left by the safe areas and the keyboard.
                .onGeometryChange(for: CGRect.self, of: { $0.frame(in: .global) }, action: { screen = $0 })
                .ignoresSafeArea()
            if unindexed {
                ContentUnavailableView("新 Session", systemImage: "text.bubble", description: Text("发送第一条提示词后显示对话"))
            } else if let url = transcriptURL {
                TranscriptWebView(
                    url: url,
                    reloadToken: reloadToken,
                    onReady: { phase = .ready },
                    onFailure: { phase = .failed($0) },
                    onCopy: copy,
                    bottomInset: bottomInset,
                    topInset: headerBottom + 8,
                    onAtBottom: { atBottom = $0 }
                )
                .ignoresSafeArea()
                .opacity(phase == .ready ? 1 : 0)
            }
            switch unindexed ? .ready : phase {
            case .loading:
                ProgressView()
            case .failed(let message):
                ContentUnavailableView {
                    Label("Transcript 加载失败", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(message)
                } actions: {
                    Button("重试", action: reload)
                        .buttonStyle(PillButtonStyle(height: 50))
                        .frame(width: 220)
                }
            case .ready:
                EmptyView()
            }
            // The status bar stays readable over the Transcript that scrolls under it.
            VStack {
                LinearGradient(
                    stops: [.init(color: Palette.transcriptGround, location: 0.6), .init(color: .clear, location: 1)],
                    startPoint: .top,
                    endPoint: .bottom
                )
                .frame(height: max(0, headerBottom - screen.minY - 49))
                Spacer(minLength: 0)
            }
            .ignoresSafeArea()
            .allowsHitTesting(false)
            if hasRequests { RequestScrim() }
        }
        .onGeometryChange(for: CGFloat.self, of: { $0.frame(in: .global).maxY }, action: { visibleBottom = $0 })
        .toolbar(.hidden, for: .navigationBar)
        .overlay(alignment: .top) {
            VStack(spacing: 8) {
                FloatingHeader(title: title, status: headerStatus) {
                    ConnectMenu(sessionId: sessionId, detail: detail, title: summary?.title) { showingInfo = true }
                }
                .onGeometryChange(for: CGFloat.self, of: { $0.frame(in: .global).maxY }, action: { headerBottom = $0 })
                if showingCopied {
                    Label("已复制", systemImage: "checkmark")
                        .font(.subheadline.weight(.medium))
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .glassCapsule()
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
        }
        .overlay(alignment: .bottom) {
            ConnectBarSlot(
                sessionId: sessionId,
                detail: detail,
                atBottom: atBottom,
                panelMaxHeight: RequestPanel.heightLimit(screen: screen.height, belowHeader: visibleBottom - headerBottom - 8)
            )
            .onGeometryChange(for: CGFloat.self, of: { $0.frame(in: .global).minY }, action: { slotTop = $0 })
        }
        .sensoryFeedback(.success, trigger: copied)
        .sheet(isPresented: $showingInfo) {
            if let detail { SessionInfoSheet(detail: detail, project: model.project(of: detail.summary)) }
        }
        .task(id: sessionId) { await loadDetail() }
        .onChange(of: unindexed) { was, now in
            // Indexed after its first prompt: show its transcript.
            if was, !now { reload() }
        }
        .onChange(of: model.resyncGeneration) {
            // The page refetches by itself after a reconnect (its own socket's `hello`): only a failed load starts over.
            if case .failed = phase { reload() } else { Task { await loadDetail() } }
        }
        .onAppear { model.connect.pageOpened(sessionId) }
        .onDisappear { model.connect.pageClosed(sessionId) }
        .onChange(of: sessionId) { old, new in
            model.connect.pageClosed(old)
            model.connect.pageOpened(new)
        }
        .onChange(of: model.connect.moved[sessionId]) { _, next in
            // `/clear` and the like: the page follows its Connect, and the input box keeps what was typed.
            guard let next else { return }
            model.connect.moved[sessionId] = nil
            detail = nil
            phase = .loading
            sessionId = next
        }
    }

    private var transcriptURL: URL? {
        guard let address = model.address else { return nil }
        var c = URLComponents(url: address.baseURL, resolvingAgainstBaseURL: false)!
        c.path = "/web/transcript.html"
        c.queryItems = [URLQueryItem(name: "sessionId", value: sessionId)]
        return c.url
    }

    private func loadDetail() async {
        guard let client = model.client else { return }
        let requested = sessionId
        // The page may have moved on (`/clear`) while this was on its way.
        guard let d = try? await client.getSession(requested), requested == sessionId else { return }
        // A Session carried on in another one opens that one, as on the desktop.
        if let next = d.supersededBy, next != sessionId {
            phase = .loading
            sessionId = next
            return
        }
        detail = d
    }

    private func reload() {
        phase = .loading
        reloadToken += 1
        Task { await loadDetail() }
    }

    private func copy(_ text: String) {
        UIPasteboard.general.string = text
        copied += 1
        withAnimation { showingCopied = true }
        Task {
            try? await Task.sleep(for: .seconds(1.2))
            withAnimation { showingCopied = false }
        }
    }
}

struct SessionInfoSheet: View {
    let detail: SessionDetail
    let project: ProjectSummary?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    LabeledContent("Project", value: project?.displayName ?? detail.projectPath)
                    if let cwd = detail.cwd { LabeledContent("目录", value: cwd) }
                    if let branch = detail.summary.gitBranch { LabeledContent("Git 分支", value: branch) }
                    if let model = detail.summary.primaryModel { LabeledContent("模型", value: model) }
                    LabeledContent("消息", value: "\(detail.summary.messageCount) 条")
                }
                Section {
                    LabeledContent("Session ID") {
                        Text(detail.summary.id).font(.footnote.monospaced()).textSelection(.enabled)
                    }
                }
            }
            .navigationTitle(detail.summary.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成", systemImage: "checkmark") { dismiss() }.labelStyle(.iconOnly)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }
}
