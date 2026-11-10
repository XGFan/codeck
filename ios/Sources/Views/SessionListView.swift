import SwiftUI

/// Pushed route to a Session's detail page.
struct SessionRoute: Hashable {
    var id: String
}

struct SessionListView: View {
    @Environment(HostModel.self) private var model
    @State private var query = ""
    @State private var changingHost = false
    @State private var path = NavigationPath()
    @State private var creating = false
    /// The order 等你处理 was last shown in.
    @State private var pendingOrder: [String] = []

    var body: some View {
        NavigationStack(path: $path) {
            TimelineView(.everyMinute) { context in
                content(now: context.date)
            }
            .background(Color(uiColor: .systemGroupedBackground))
            .overlay(alignment: .bottomTrailing) { newButton }
            .navigationTitle("Session")
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .automatic), prompt: "搜索标题或 Project")
            .toolbar { toolbar }
            .navigationDestination(for: SessionRoute.self) { route in
                SessionDetailView(sessionId: route.id)
            }
            .navigationDestination(for: StartingRoute.self) { StartingView(route: $0) }
            .sheet(isPresented: $creating) {
                NewSessionSheet { conn, cwd in path.append(StartingRoute(conn: conn, cwd: cwd)) }
            }
            .onChange(of: model.connect.openRequest) { _, request in
                // A Fork got its Session: open it over the Origin's page.
                guard let request else { return }
                model.connect.openRequest = nil
                path.append(SessionRoute(id: request.sessionId))
            }
            .sheet(isPresented: $changingHost) {
                NavigationStack {
                    HostSetupView(current: model.address) { address in
                        path = NavigationPath()
                        model.connect(to: address)
                    }
                }
            }
        }
    }

    private func content(now: Date) -> some View {
        let all = model.filtered(query)
        let searching = !query.trimmingCharacters(in: .whitespaces).isEmpty
        return ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if searching {
                    if let problem = model.problem {
                        OfflineCard(message: problem, onRetry: model.retry) { changingHost = true }
                    }
                    if !all.isEmpty {
                        card(all) { SessionSearchRow(session: $0, project: model.project(of: $0), now: now) }
                    }
                } else {
                    sections(all, now: now)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 88)
            .coordinateSpace(.named(PendingCard.listSpace))
        }
        .refreshable { await model.refresh() }
        .overlay {
            if all.isEmpty {
                if searching {
                    ContentUnavailableView.search(text: query)
                } else if !model.hasLoaded, model.problem == nil {
                    ProgressView()
                } else if model.hasLoaded {
                    ContentUnavailableView("没有 Session", systemImage: "text.bubble", description: Text("Host 的数据目录里还没有 Session。"))
                }
            }
        }
    }

    @ViewBuilder
    private func sections(_ all: [SessionSummary], now: Date) -> some View {
        let store = model.connect
        let s = SessionSections(all, pendingOrder: pendingOrder) { id in
            store.conn(for: id).map { $0.isLive && !store.requests(of: $0).isEmpty } ?? false
        }
        if let problem = model.problem {
            OfflineCard(message: problem, onRetry: model.retry) { changingHost = true }
        }
        Group {
            if !s.pending.isEmpty {
                section("等你处理 \(s.pending.count)", color: Palette.warnText) {
                    VStack(spacing: 8) {
                        ForEach(s.pending) { PendingCard(session: $0, project: model.project(of: $0)) }
                    }
                }
            }
            if !s.running.isEmpty {
                section("进行中") {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible())], spacing: 8) {
                        ForEach(s.running) { SessionRunningCard(session: $0, project: model.project(of: $0), now: now) }
                    }
                }
            }
            if !s.recent.isEmpty {
                section("最近") {
                    card(s.recent) { SessionRecentRow(session: $0, now: now) }
                }
            }
        }
        .opacity(model.problem == nil ? 1 : 0.55)
        .animation(.default, value: s)
        .onChange(of: s.pending.map(\.id), initial: true) { _, order in pendingOrder = order }
    }

    private func section<C: View>(_ title: String, color: Color = .secondary, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.footnote.weight(.semibold)).foregroundStyle(color).padding(.horizontal, 4)
            content()
        }
    }

    /// Rows in one white card, with hairlines between.
    private func card<Row: View>(_ items: [SessionSummary], @ViewBuilder row: @escaping (SessionSummary) -> Row) -> some View {
        LazyVStack(spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                if i > 0 { Divider().padding(.leading, 16) }
                row(item)
            }
        }
        .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 18))
        .clipShape(RoundedRectangle(cornerRadius: 18))
    }

    private var newButton: some View {
        Button { creating = true } label: {
            Image(systemName: "plus")
                .font(.system(size: 22, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 56, height: 56)
                .background(Palette.accent, in: Circle())
                .shadow(color: Palette.accent.opacity(0.35), radius: 10, y: 6)
        }
        .disabled(!model.hasLoaded)
        .opacity(model.hasLoaded ? 1 : 0.5)
        .padding(.trailing, 16)
        .padding(.bottom, 16)
        .accessibilityLabel("新建 Session")
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
                Image(systemName: "desktopcomputer")
                    .overlay(alignment: .topTrailing) {
                        Circle()
                            .fill(model.problem == nil ? Palette.ok : Palette.warn)
                            .frame(width: 9, height: 9)
                            .offset(x: 4, y: -3)
                    }
                    .foregroundStyle(model.problem == nil ? Color.primary : Palette.warnText)
                    .accessibilityLabel("Host")
            }
        }
    }
}

/// 连不上 Host: what happened, and what to do about it.
struct OfflineCard: View {
    let message: String
    let onRetry: () -> Void
    let onChangeHost: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(message).font(.subheadline.weight(.semibold)).foregroundStyle(Palette.warnText)
            HStack(spacing: 8) {
                Button("重试", action: onRetry).buttonStyle(PillButtonStyle(height: 44))
                Button("更换 Host", action: onChangeHost).buttonStyle(PillButtonStyle(kind: .tinted, height: 44))
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 18))
    }
}
