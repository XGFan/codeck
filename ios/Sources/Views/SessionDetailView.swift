import SwiftUI

/// One Session: the Host's Transcript page, with native loading / error states.
struct SessionDetailView: View {
    @Environment(HostModel.self) private var model
    @State private var sessionId: String
    @State private var detail: SessionDetail?
    @State private var phase = Phase.loading
    @State private var reloadToken = 0
    @State private var copied = 0
    @State private var showingCopied = false
    @State private var showingInfo = false

    enum Phase: Equatable {
        case loading, ready
        case failed(String)
    }

    init(sessionId: String) {
        _sessionId = State(initialValue: sessionId)
    }

    private var summary: SessionSummary? { model.session(id: sessionId) ?? detail?.summary }

    var body: some View {
        ZStack {
            Color(uiColor: .systemBackground).ignoresSafeArea()
            if let url = transcriptURL {
                TranscriptWebView(
                    url: url,
                    reloadToken: reloadToken,
                    onReady: { phase = .ready },
                    onFailure: { phase = .failed($0) },
                    onCopy: copy
                )
                .ignoresSafeArea(edges: .bottom)
                .opacity(phase == .ready ? 1 : 0)
            }
            switch phase {
            case .loading:
                ProgressView()
            case .failed(let message):
                ContentUnavailableView {
                    Label("Transcript 加载失败", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(message)
                } actions: {
                    Button("重试", action: reload).buttonStyle(.borderedProminent)
                }
            case .ready:
                EmptyView()
            }
        }
        .navigationTitle(summary?.title ?? "Session")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("详情", systemImage: "info.circle") { showingInfo = true }
                    .disabled(detail == nil)
            }
            ToolbarItem(placement: .topBarTrailing) {
                ConnectMenu(sessionId: sessionId)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            ConnectBarSlot(sessionId: sessionId)
        }
        .overlay(alignment: .bottom) {
            if showingCopied {
                Label("已复制", systemImage: "checkmark")
                    .font(.subheadline.weight(.medium))
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(.regularMaterial, in: Capsule())
                    .padding(.bottom, 24)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .sensoryFeedback(.success, trigger: copied)
        .sheet(isPresented: $showingInfo) {
            if let detail { SessionInfoSheet(detail: detail, project: model.project(of: detail.summary)) }
        }
        .task(id: sessionId) { await loadDetail() }
        .onChange(of: model.resyncGeneration) {
            // After a reconnect the page reloads in place; the loading state shows only after a failure.
            if case .failed = phase { phase = .loading }
            reloadToken += 1
            Task { await loadDetail() }
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
        guard let d = try? await client.getSession(sessionId) else { return }
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
                ToolbarItem(placement: .confirmationAction) { Button("完成") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }
}
