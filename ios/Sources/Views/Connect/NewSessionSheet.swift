import SwiftUI

/// 「新建 Session」: pick a Project or type a directory on the Host (completed from its file system), then
/// `ccrpc_new` starts claude there in a new clay Pane and Connects it.
struct NewSessionSheet: View {
    /// The connection started and the directory it starts in.
    let onStarted: (_ conn: String, _ cwd: String) -> Void
    @Environment(HostModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var path = ""
    @State private var sidebar: [SidebarProject]?
    @State private var completions: [String] = []
    @State private var starting = false
    @State private var error: String?
    @FocusState private var typing: Bool

    private var cwd: String { StartFlow.cwd(from: path) }

    var body: some View {
        let directories = StartFlow.directories(sidebar: sidebar, projects: Array(model.projects.values))
        NavigationStack {
            List {
                Section {
                    TextField("目录，例如 /Users/me/work/app", text: $path)
                        .font(.body.monospaced())
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.go)
                        .focused($typing)
                        .onSubmit(start)
                    if typing {
                        ForEach(completions, id: \.self) { c in
                            Button {
                                path = c
                            } label: {
                                Label(c, systemImage: "folder").font(.callout.monospaced()).lineLimit(1)
                            }
                        }
                    }
                } header: {
                    Text("目录")
                } footer: {
                    if let error {
                        Text(error).foregroundStyle(.red)
                    } else if let problem = StartFlow.cwdProblem(path) {
                        Text(problem)
                    }
                }
                if !directories.isEmpty {
                    Section("Project") {
                        ForEach(directories) { d in
                            Button {
                                path = d.path
                                typing = false
                            } label: {
                                HStack {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(d.name).foregroundStyle(.primary)
                                        Text(d.path).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
                                            .truncationMode(.head)
                                    }
                                    Spacer()
                                    if cwd == d.path { Image(systemName: "checkmark").foregroundStyle(.tint) }
                                }
                            }
                        }
                    }
                }
            }
            .navigationTitle("新建 Session")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("取消") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    if starting {
                        ProgressView()
                    } else {
                        Button("新建", action: start).disabled(cwd.isEmpty || StartFlow.cwdProblem(path) != nil)
                    }
                }
            }
            .task { sidebar = try? await model.client?.sidebarProjects() }
            .task(id: path) { await complete() }
        }
        .presentationDetents([.large])
    }

    private func complete() async {
        let partial = path
        // The Host completes full paths only.
        guard partial.hasPrefix("/") else { return completions = [] }
        try? await Task.sleep(for: .milliseconds(200))
        guard !Task.isCancelled, let found = try? await model.client?.completeDirectories(partial) else { return }
        completions = found.filter { $0 != partial }
    }

    private func start() {
        let cwd = cwd
        guard !cwd.isEmpty, StartFlow.cwdProblem(path) == nil, !starting else { return }
        starting = true
        error = nil
        Task {
            defer { starting = false }
            do {
                let conn = try await model.connect.startNew(cwd: cwd)
                dismiss()
                onStarted(conn, cwd)
            } catch {
                self.error = "新建 Session 失败：\(ConnectStore.text(error))"
            }
        }
    }
}

/// Pushed route to a Session being started (`ccrpc_new`).
struct StartingRoute: Hashable {
    var conn: String
    var cwd: String
}

/// 「正在启动…」 until claude names the Session, then that Session's page; what claude asks before that (the folder
/// trust dialog) is answered here.
struct StartingView: View {
    let route: StartingRoute
    @Environment(HostModel.self) private var model

    var body: some View {
        if let sessionId = model.connect.started[route.conn] {
            SessionDetailView(sessionId: sessionId)
        } else {
            Group {
                if let failure = model.connect.startFailures[route.conn] {
                    ContentUnavailableView {
                        Label("新建 Session 失败", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(failure)
                    }
                } else {
                    VStack(spacing: 12) {
                        ProgressView()
                        Text("正在启动…").font(.headline)
                        Text(route.cwd).font(.footnote.monospaced()).foregroundStyle(.secondary)
                        if let c = model.connect.conns[route.conn] {
                            let requests = model.connect.requests(of: c)
                            if !requests.isEmpty {
                                ScrollView {
                                    VStack(spacing: 8) {
                                        ForEach(requests) { RequestCard(request: $0, c: c) }
                                    }
                                }
                                .scrollBounceBehavior(.basedOnSize)
                            }
                        }
                    }
                    .padding()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(uiColor: .systemBackground))
            .navigationTitle("新 Session")
            .navigationBarTitleDisplayMode(.inline)
        }
    }
}
