import SwiftUI

/// Pushed route to a Session's detail page.
struct SessionRoute: Hashable {
    var id: String
}

struct SessionListView: View {
    @Environment(HostModel.self) private var model
    @State private var query = ""
    @State private var changingHost = false
    @State private var path: [SessionRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            TimelineView(.everyMinute) { context in
                list(now: context.date)
            }
            .navigationTitle("Session")
            .searchable(text: $query, prompt: "搜索标题或 Project")
            .refreshable { await model.refresh() }
            .toolbar { toolbar }
            .navigationDestination(for: SessionRoute.self) { route in
                SessionDetailView(sessionId: route.id)
            }
            .sheet(isPresented: $changingHost) {
                NavigationStack {
                    HostSetupView(current: model.address) { address in
                        path = []
                        model.connect(to: address)
                    }
                }
            }
        }
    }

    private func list(now: Date) -> some View {
        let all = model.filtered(query)
        let live = all.filter { $0.live != nil }.sorted {
            let (a, b) = ($0.live!.status.urgency, $1.live!.status.urgency)
            return a != b ? a > b : $0.lastActiveMs > $1.lastActiveMs
        }
        let rest = all.filter { $0.live == nil }.sorted { $0.lastActiveMs > $1.lastActiveMs }

        return List {
            if let problem = model.problem {
                Section {
                    OfflineBanner(message: problem, onRetry: model.retry) { changingHost = true }
                }
            }
            if !live.isEmpty {
                Section("Live") {
                    ForEach(live) { row($0, now: now) }
                }
            }
            if !rest.isEmpty {
                Section("全部") {
                    ForEach(rest) { row($0, now: now) }
                }
            }
        }
        .listStyle(.insetGrouped)
        .overlay {
            if all.isEmpty {
                if !query.isEmpty {
                    ContentUnavailableView.search(text: query)
                } else if !model.hasLoaded, model.problem == nil {
                    ProgressView()
                } else if model.hasLoaded {
                    ContentUnavailableView("没有 Session", systemImage: "text.bubble", description: Text("Host 的数据目录里还没有 Session。"))
                }
            }
        }
        .animation(.default, value: live.map(\.id))
    }

    private func row(_ session: SessionSummary, now: Date) -> some View {
        NavigationLink(value: SessionRoute(id: session.id)) {
            SessionRow(session: session, project: model.project(of: session), now: now)
        }
        .contextMenu {
            Button("复制 Session ID", systemImage: "doc.on.doc") {
                UIPasteboard.general.string = session.id
            }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                if let address = model.address {
                    Section("Host") {
                        Text(address.display)
                    }
                }
                Button("更换 Host", systemImage: "network") { changingHost = true }
            } label: {
                Image(systemName: model.problem == nil ? "desktopcomputer" : "exclamationmark.icloud")
                    .accessibilityLabel("Host")
            }
        }
    }
}

struct OfflineBanner: View {
    let message: String
    let onRetry: () -> Void
    let onChangeHost: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(message, systemImage: "wifi.exclamationmark")
                .foregroundStyle(.orange)
            HStack(spacing: 12) {
                Button("重试", action: onRetry)
                    .buttonStyle(.borderedProminent)
                Button("更换 Host", action: onChangeHost)
                    .buttonStyle(.bordered)
            }
            .controlSize(.small)
        }
        .padding(.vertical, 4)
    }
}
