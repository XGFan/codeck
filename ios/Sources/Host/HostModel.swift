import Foundation
import Observation
import UIKit

/// Dependencies for `HostModel.executePairing`, injectable for unit tests.
struct PairingDependencies: Sendable {
    var checkHealth: @Sendable (HostAddress, String, TimeInterval) async throws -> HostHealth
    var checkToken: @Sendable (HostAddress, String, String) async throws -> Void
    var pair: @Sendable (HostAddress, String, String, String) async throws -> (deviceId: String, token: String)
    var save: @Sendable (PairedHost) throws -> Void

    static func live(store: HostStore) -> PairingDependencies {
        PairingDependencies(
            checkHealth: { address, fingerprint, timeout in
                try await HostClient(address: address, fingerprint: fingerprint).checkHealth(timeout: timeout)
            },
            checkToken: { address, fingerprint, token in
                try await HostClient(address: address, fingerprint: fingerprint, token: token).checkAuth(timeout: 5)
            },
            pair: { address, fingerprint, key, name in
                try await HostClient(address: address, fingerprint: fingerprint).pair(key: key, name: name)
            },
            save: { host in
                try store.save(host)
            }
        )
    }
}

/// App state for one Host: the Session list, Projects, Live states, and the event stream.
@MainActor
@Observable
final class HostModel {
    private(set) var host: PairedHost?
    var address: HostAddress? { host?.address }
    private(set) var sessions: [SessionSummary] = []
    private(set) var projects: [String: ProjectSummary] = [:]
    private(set) var hasLoaded = false
    private(set) var isLoading = false
    /// What went wrong with the Host; shown as the offline banner.
    private(set) var problem: String?
    /// Terminal auth/certificate error: stays set until the next successful pairing.
    private(set) var needsPairing = false
    /// Prevents concurrent pairing attempts.
    private(set) var isPairing = false
    /// Increments on each `hello` after the first, i.e. after every reconnect: open pages catch up.
    private(set) var resyncGeneration = 0
    /// Increments on every Host switch; in-flight work from older generations is discarded.
    private(set) var generation = 0
    /// Current Host's HTTP client (shares the pinned session with the event stream).
    private(set) var client: HostClient?
    /// The Host's Connects and `ccrpc ls`.
    let connect = ConnectStore()

    @ObservationIgnored private var rawEventSubscribers: [UUID: (String) -> Void] = [:]
    @ObservationIgnored private let store: HostStore
    @ObservationIgnored private let makeClient: (PairedHost, PinnedSession) -> HostClient
    @ObservationIgnored private let makeStream: (PairedHost, PinnedSession) -> EventStream
    @ObservationIgnored private var stream: EventStream?
    @ObservationIgnored private var helloCount = 0
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var attributionTask: Task<Void, Never>?
    /// Monotonic token per disconnect/reconnect cycle; valid `hello` increments it to invalidate in-flight probes.
    @ObservationIgnored private var disconnectEpoch = 0
    @ObservationIgnored private var refreshAgain = false
    @ObservationIgnored private var checkHealthNext = false
    /// Changes come a few times a second while a Session writes; each refetch is the whole list.
    @ObservationIgnored private lazy var eventRefresh = TrailingThrottle(interval: .seconds(1)) { [weak self] in
        await self?.refresh(checkHealth: false)
    }

    init(
        store: HostStore = HostStore(),
        clientFactory: @escaping (PairedHost, PinnedSession) -> HostClient = { host, pinned in
            HostClient(host: host, pinnedSession: pinned)
        },
        streamFactory: @escaping (PairedHost, PinnedSession) -> EventStream = { host, pinned in
            EventStream(host: host, pinnedSession: pinned)
        }
    ) {
        self.store = store
        self.makeClient = clientFactory
        self.makeStream = streamFactory
        if let saved = store.load() { use(saved) }
    }

    /// Subscribes to raw WebSocket event text for the current Host; returns a cancellation closure.
    func subscribeRawEvents(_ handler: @escaping (String) -> Void) -> () -> Void {
        let id = UUID()
        rawEventSubscribers[id] = handler
        return { [weak self] in
            self?.rawEventSubscribers.removeValue(forKey: id)
        }
    }

    /// Delivers a raw event text to all active subscribers (used by `EventStream` and tests).
    func emitRawEvent(_ text: String) {
        guard !needsPairing else { return }
        for handler in rawEventSubscribers.values {
            handler(text)
        }
    }

    /// DEBUG-only plaintext loopback path (`127.0.0.1`, no fingerprint, no token).
    func connect(to address: HostAddress) {
        guard store.allowLoopback, address.host == "127.0.0.1" else { return }
        let loopback = PairedHost(address: address, fingerprint: nil, deviceId: nil, token: nil)
        guard (try? store.save(loopback)) != nil else { return }
        use(loopback)
    }

    /// Probes `link.addresses` sequentially with unauthenticated pinned `/health` (3s each) and returns
    /// the first reachable address without sending any credentials.
    func resolve(_ link: PairingLink, dependencies: PairingDependencies? = nil) async throws -> HostAddress {
        let deps = dependencies ?? .live(store: store)
        return try await Self.resolveAddress(for: link, dependencies: deps)
    }

    /// Pairs with `link` at a previously resolved `address`. Calls while pairing is in progress are ignored.
    @discardableResult
    func pair(with link: PairingLink, at address: HostAddress, dependencies: PairingDependencies? = nil) async throws -> Bool {
        guard !isPairing else { return false }
        isPairing = true
        defer { isPairing = false }
        let deps = dependencies ?? .live(store: store)
        let paired = try await Self.executePairing(
            link: link,
            at: address,
            existing: host,
            deviceName: UIDevice.current.name,
            dependencies: deps
        )
        try Task.checkCancellation()
        use(paired)
        return true
    }

    /// Pairs with a Host from a `codeck://pair` link (in-app QR scan path: resolves then pairs directly).
    /// Calls while pairing is in progress are ignored.
    @discardableResult
    func pair(with link: PairingLink, dependencies: PairingDependencies? = nil) async throws -> Bool {
        guard !isPairing else { return false }
        isPairing = true
        defer { isPairing = false }
        let deps = dependencies ?? .live(store: store)
        let address = try await Self.resolveAddress(for: link, dependencies: deps)
        try Task.checkCancellation()
        let paired = try await Self.executePairing(
            link: link,
            at: address,
            existing: host,
            deviceName: UIDevice.current.name,
            dependencies: deps
        )
        try Task.checkCancellation()
        use(paired)
        return true
    }

    /// Step 1 of pairing: probe `link.addresses` sequentially with 3s unauthenticated pinned `/health`.
    static func resolveAddress(
        for link: PairingLink,
        dependencies deps: PairingDependencies
    ) async throws -> HostAddress {
        var versionError: HostError?
        for candidate in link.addresses {
            try Task.checkCancellation()
            do {
                _ = try await deps.checkHealth(candidate, link.fingerprint, 3)
                return candidate
            } catch is CancellationError {
                throw CancellationError()
            } catch let error as HostError {
                if Task.isCancelled { throw CancellationError() }
                if case .versionMismatch = error, versionError == nil {
                    versionError = error
                }
            } catch {
                if Task.isCancelled { throw CancellationError() }
            }
        }
        throw versionError ?? HostError.pairingUnreachable
    }

    /// Steps 2–3 of pairing against a previously resolved `address`:
    /// 1. If the saved Host has the same fingerprint, verify its token via `get_app_info`:
    ///    - success → persist updated address only (skip `/v1/pair`; save error propagates as-is);
    ///    - explicit 401 (`.removed`) → proceed to `/v1/pair`;
    ///    - any other failure → fail with `.pairingUnreachable` without consuming the key.
    /// 2. Otherwise call `/v1/pair`, persist via `save`, and return the new `PairedHost`.
    /// Checks `Task.isCancelled` before `/v1/pair` and before `save`.
    static func executePairing(
        link: PairingLink,
        at address: HostAddress,
        existing: PairedHost?,
        deviceName: String,
        dependencies deps: PairingDependencies
    ) async throws -> PairedHost {
        try Task.checkCancellation()

        if let existing,
           existing.fingerprint == link.fingerprint,
           let existingDeviceId = existing.deviceId,
           let existingToken = existing.token {
            let tokenValid: Bool
            do {
                try await deps.checkToken(address, link.fingerprint, existingToken)
                tokenValid = true
            } catch is CancellationError {
                throw CancellationError()
            } catch HostError.removed {
                // Token was revoked on the Host: fall through to fresh pairing.
                tokenValid = false
            } catch {
                if Task.isCancelled { throw CancellationError() }
                // Transient or unexpected error: do not consume the one-time pairing key.
                throw HostError.pairingUnreachable
            }
            if tokenValid {
                try Task.checkCancellation()
                let updated = PairedHost(
                    address: address,
                    fingerprint: link.fingerprint,
                    deviceId: existingDeviceId,
                    token: existingToken
                )
                try deps.save(updated)
                return updated
            }
        }

        try Task.checkCancellation()
        let credentials = try await deps.pair(address, link.fingerprint, link.key, deviceName)
        try Task.checkCancellation()
        let paired = PairedHost(
            address: address,
            fingerprint: link.fingerprint,
            deviceId: credentials.deviceId,
            token: credentials.token
        )
        try deps.save(paired)
        return paired
    }

    /// Runs the full pairing state machine (`resolveAddress` + `executePairing(link:at:...)`).
    static func executePairing(
        link: PairingLink,
        existing: PairedHost?,
        deviceName: String,
        dependencies deps: PairingDependencies
    ) async throws -> PairedHost {
        let address = try await resolveAddress(for: link, dependencies: deps)
        return try await executePairing(
            link: link,
            at: address,
            existing: existing,
            deviceName: deviceName,
            dependencies: deps
        )
    }

    private func use(_ paired: PairedHost) {
        generation += 1
        let gen = generation
        disconnectEpoch += 1
        stream?.stop()
        eventRefresh.cancel()
        refreshTask?.cancel()
        refreshTask = nil
        refreshAgain = false
        checkHealthNext = false
        attributionTask?.cancel()
        attributionTask = nil

        host = paired
        needsPairing = false
        sessions = []
        projects = [:]
        hasLoaded = false
        problem = nil
        helloCount = 0

        let pinned = PinnedSession(expectedFingerprint: paired.fingerprint)
        let client = makeClient(paired, pinned)
        self.client = client
        connect.reset(client: client)

        let stream = makeStream(paired, pinned)
        stream.onRawMessage = { [weak self] text in
            guard let self, self.generation == gen, !self.needsPairing else { return }
            self.emitRawEvent(text)
        }
        stream.onEvent = { [weak self] event in
            guard let self, self.generation == gen, !self.needsPairing else { return }
            self.handle(event, generation: gen)
        }
        stream.onDisconnect = { [weak self] attempt, certMismatch in
            guard let self, self.generation == gen, !self.needsPairing else { return }
            self.handleDisconnect(attempt: attempt, certMismatch: certMismatch, generation: gen, client: client)
        }
        self.stream = stream
        stream.start()
        Task { await refresh() }
    }

    /// Called when the Transcript web bridge observes an HTTP 401 from the bound Host.
    func handleBridgeUnauthorized(generation gen: Int) {
        guard generation == gen, !needsPairing else { return }
        enterNeedsPairing(.removed, generation: gen)
    }

    /// Attributes consecutive WebSocket disconnects via `get_app_info` (401 vs cert change vs unreachable).
    func handleDisconnect(attempt: Int, certMismatch: Bool, generation gen: Int, client: HostClient) {
        guard generation == gen, !needsPairing else { return }
        if certMismatch {
            enterNeedsPairing(.certificateChanged, generation: gen)
            return
        }
        // A single drop reconnects within a second; only probe and report one that keeps failing.
        guard attempt >= 1 else { return }
        disconnectEpoch += 1
        let epoch = disconnectEpoch
        attributionTask?.cancel()
        attributionTask = Task { [weak self] in
            guard let self else { return }
            do {
                try await client.checkAuth(timeout: 5)
                guard !Task.isCancelled, self.generation == gen, self.disconnectEpoch == epoch, !self.needsPairing else { return }
                self.problem = HostError.unreachable(client.address.display).errorDescription
            } catch is CancellationError {
                return
            } catch let error as HostError {
                guard !Task.isCancelled, self.generation == gen, self.disconnectEpoch == epoch, !self.needsPairing else { return }
                switch error {
                case .removed:
                    self.enterNeedsPairing(.removed, generation: gen)
                case .certificateChanged:
                    self.enterNeedsPairing(.certificateChanged, generation: gen)
                default:
                    self.problem = HostError.unreachable(client.address.display).errorDescription
                }
            } catch {
                guard !Task.isCancelled, self.generation == gen, self.disconnectEpoch == epoch, !self.needsPairing else { return }
                self.problem = HostError.unreachable(client.address.display).errorDescription
            }
        }
    }

    private func enterNeedsPairing(_ error: HostError, generation gen: Int) {
        guard generation == gen else { return }
        disconnectEpoch += 1
        needsPairing = true
        problem = error.errorDescription
        stream?.stop()
        eventRefresh.cancel()
        attributionTask?.cancel()
        attributionTask = nil
    }

    // MARK: Lifecycle

    /// The app came to the foreground: reconnect; the `hello` triggers the refetch.
    func resume() {
        guard !needsPairing, let stream, !stream.isRunning else { return }
        stream.start()
    }

    /// The app went to the background: close the socket.
    func pause() {
        stream?.stop()
    }

    /// 「重试」.
    func retry() {
        guard !needsPairing else { return }
        stream?.restart()
        Task { await refresh() }
    }

    // MARK: Fetching

    /// Refetches Sessions and Projects; concurrent calls coalesce into one more pass.
    func refresh(checkHealth: Bool = true) async {
        guard !needsPairing else { return }
        checkHealthNext = checkHealthNext || checkHealth
        if let refreshTask {
            refreshAgain = true
            await refreshTask.value
            return
        }
        let task = Task {
            repeat {
                refreshAgain = false
                let health = checkHealthNext
                checkHealthNext = false
                await fetchAll(checkHealth: health)
            } while refreshAgain && !Task.isCancelled && !needsPairing
        }
        refreshTask = task
        await task.value
        if refreshTask == task { refreshTask = nil }
    }

    private func fetchAll(checkHealth: Bool) async {
        guard !needsPairing, let client else { return }
        let gen = generation
        isLoading = true
        defer { if generation == gen { isLoading = false } }
        do {
            if checkHealth { _ = try await client.checkHealth() }
            async let sessions = client.listSessions()
            async let projects = client.listProjects()
            let (s, p) = try await (sessions, projects)
            guard generation == gen, !needsPairing else { return }
            self.sessions = s
            self.projects = Dictionary(p.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
            hasLoaded = true
            problem = nil
        } catch let error as HostError {
            guard generation == gen, !needsPairing else { return }
            switch error {
            case .removed:
                enterNeedsPairing(.removed, generation: gen)
            case .certificateChanged:
                enterNeedsPairing(.certificateChanged, generation: gen)
            default:
                problem = error.errorDescription
            }
        } catch {
            guard generation == gen, !needsPairing else { return }
            problem = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    // MARK: Events

    func handle(_ event: HostEvent) {
        handle(event, generation: generation)
    }

    private func handle(_ event: HostEvent, generation gen: Int) {
        guard generation == gen, !needsPairing else { return }
        switch event {
        case .hello(let apiVersion):
            guard apiVersion == HostClient.apiVersion else {
                disconnectEpoch += 1
                attributionTask?.cancel()
                attributionTask = nil
                stream?.stop()
                problem = HostError.versionMismatch(host: apiVersion).errorDescription
                return
            }
            // Valid reconnect supersedes any in-flight disconnect attribution probe.
            disconnectEpoch += 1
            attributionTask?.cancel()
            attributionTask = nil
            helloCount += 1
            if helloCount > 1 { resyncGeneration += 1 }
            Task { await refresh() }
            Task { await connect.resync() }
        case .sessionsChanged:
            // list_sessions has no id filter; the whole list is one call.
            eventRefresh.fire()
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
            eventRefresh.fire()
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

/// Runs `action` `interval` after the first `fire()`; fires until then are covered by that run, later ones start
/// the next interval, so the last change is never lost.
@MainActor
final class TrailingThrottle {
    private let interval: Duration
    private let action: @MainActor () async -> Void
    private var pending: Task<Void, Never>?

    init(interval: Duration, action: @escaping @MainActor () async -> Void) {
        self.interval = interval
        self.action = action
    }

    func fire() {
        guard pending == nil else { return }
        pending = Task { [weak self, interval] in
            try? await Task.sleep(for: interval)
            guard let self, !Task.isCancelled else { return }
            self.pending = nil
            await self.action()
        }
    }

    func cancel() {
        pending?.cancel()
        pending = nil
    }
}
