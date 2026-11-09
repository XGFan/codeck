import Foundation
import Observation

/// App state for one Host: the Session list, Projects, Live states, and the event stream.
@MainActor
@Observable
final class HostModel {
    private(set) var address: HostAddress?
    private(set) var sessions: [SessionSummary] = []
    private(set) var projects: [String: ProjectSummary] = [:]
    private(set) var hasLoaded = false
    private(set) var isLoading = false
    /// What went wrong with the Host; shown as the offline banner.
    private(set) var problem: String?
    /// Increments on each `hello` after the first, i.e. after every reconnect: open pages reload.
    private(set) var resyncGeneration = 0
    /// The Host's Connects and `ccrpc ls`.
    let connect = ConnectStore()

    @ObservationIgnored private let store: HostAddressStore
    @ObservationIgnored private var stream: EventStream?
    @ObservationIgnored private var helloCount = 0
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var refreshAgain = false

    init(store: HostAddressStore = HostAddressStore()) {
        self.store = store
        if let saved = store.load() { use(saved) }
    }

    var client: HostClient? { address.map { HostClient(address: $0) } }

    /// Switches to `address` (already health-checked) and saves it.
    func connect(to address: HostAddress) {
        store.save(address)
        use(address)
    }

    private func use(_ address: HostAddress) {
        stream?.stop()
        self.address = address
        sessions = []
        projects = [:]
        hasLoaded = false
        problem = nil
        helloCount = 0
        connect.reset(client: HostClient(address: address))
        let stream = EventStream(address: address)
        stream.onEvent = { [weak self] in self?.handle($0) }
        stream.onDisconnect = { [weak self] attempt in
            // A single drop reconnects within a second; only report one that keeps failing.
            if attempt >= 1 { self?.problem = HostError.unreachable(address.display).errorDescription }
        }
        self.stream = stream
        stream.start()
        Task { await refresh() }
    }

    // MARK: Lifecycle

    /// The app came to the foreground: reconnect; the `hello` triggers the refetch.
    func resume() {
        guard let stream, !stream.isRunning else { return }
        stream.start()
    }

    /// The app went to the background: close the socket.
    func pause() {
        stream?.stop()
    }

    /// 「重试」.
    func retry() {
        stream?.restart()
        Task { await refresh() }
    }

    // MARK: Fetching

    /// Refetches Sessions and Projects; concurrent calls coalesce into one more pass.
    func refresh() async {
        if let refreshTask {
            refreshAgain = true
            await refreshTask.value
            return
        }
        let task = Task {
            repeat {
                refreshAgain = false
                await fetchAll()
            } while refreshAgain
        }
        refreshTask = task
        await task.value
        refreshTask = nil
    }

    private func fetchAll() async {
        guard let client else { return }
        let address = client.address
        isLoading = true
        defer { isLoading = false }
        do {
            _ = try await client.checkHealth()
            async let sessions = client.listSessions()
            async let projects = client.listProjects()
            let (s, p) = try await (sessions, projects)
            guard address == self.address else { return }
            self.sessions = s
            self.projects = Dictionary(p.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
            hasLoaded = true
            problem = nil
        } catch {
            guard address == self.address else { return }
            problem = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    // MARK: Events

    func handle(_ event: HostEvent) {
        switch event {
        case .hello(let apiVersion):
            guard apiVersion == HostClient.apiVersion else {
                stream?.stop()
                problem = HostError.versionMismatch(host: apiVersion).errorDescription
                return
            }
            helloCount += 1
            if helloCount > 1 { resyncGeneration += 1 }
            Task { await refresh() }
            Task { await connect.resync() }
        case .sessionsChanged:
            // list_sessions has no id filter; the whole list is one call.
            Task { await refresh() }
        case .liveChanged(let snapshot):
            apply(snapshot)
        case .connectState(let state):
            connect.apply(state)
        case .connectRemoved(let conn):
            connect.remove(conn: conn)
        case .ccrpcLs(let text):
            connect.applyLs(text)
        case .ccrpcLsEnded(let message):
            connect.lsEnded(message)
        case .indexStatus, .pricesChanged, .other:
            break
        }
    }

    /// Applies a full Live snapshot; a Live Session missing from the list means the list is stale.
    func apply(_ snapshot: LiveChanged) {
        let states = Dictionary(snapshot.live.map { ($0.sessionId, $0.state) }, uniquingKeysWith: { a, _ in a })
        for i in sessions.indices where sessions[i].live != states[sessions[i].id] {
            sessions[i].live = states[sessions[i].id]
        }
        let known = Set(sessions.map(\.id))
        if hasLoaded, states.keys.contains(where: { !known.contains($0) }) {
            Task { await refresh() }
        }
    }

    // MARK: Derived

    func project(of session: SessionSummary) -> ProjectSummary? { projects[session.projectId] }

    func session(id: String) -> SessionSummary? { sessions.first { $0.id == id } }

    /// Matches the Session title and its Project's name or path, case-insensitively.
    func filtered(_ query: String) -> [SessionSummary] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return sessions }
        return sessions.filter { s in
            if s.title.localizedCaseInsensitiveContains(q) { return true }
            guard let p = projects[s.projectId] else { return false }
            return p.displayName.localizedCaseInsensitiveContains(q) || p.path.localizedCaseInsensitiveContains(q)
        }
    }
}
