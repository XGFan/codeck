import Foundation

/// `wss://<host>/v1/events` (or `ws://` for DEBUG loopback). Every `hello` (first connect and each
/// reconnect) means the client must refetch everything: the Host keeps no event history.
@MainActor
final class EventStream {
    /// Every decoded event, `hello` included.
    var onEvent: (HostEvent) -> Void = { _ in }
    /// Every raw text message received on the socket, forwarded verbatim to the Transcript page.
    var onRawMessage: (String) -> Void = { _ in }
    /// The socket closed or could not connect; `attempt` counts consecutive failures from 0.
    var onDisconnect: (_ attempt: Int, _ certificateChanged: Bool) -> Void = { _, _ in }

    static let maximumMessageSize = 64 * 1024 * 1024

    private let request: URLRequest
    private let session: URLSession
    private let pinnedSession: PinnedSession?
    private var loop: Task<Void, Never>?
    private var socket: URLSessionWebSocketTask?

    init(host: PairedHost, session: URLSession? = nil, pinnedSession: PinnedSession? = nil) {
        var request = URLRequest(url: host.eventsURL)
        if let token = host.token, !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        self.request = request
        if let session {
            self.session = session
            self.pinnedSession = pinnedSession
        } else if let pinnedSession {
            self.session = pinnedSession.session
            self.pinnedSession = pinnedSession
        } else {
            let owned = PinnedSession(expectedFingerprint: host.fingerprint)
            self.session = owned.session
            self.pinnedSession = owned
        }
    }

    var isRunning: Bool { loop != nil }

    func start() {
        guard loop == nil else { return }
        loop = Task { [weak self] in await self?.run() }
    }

    func stop() {
        loop?.cancel()
        loop = nil
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
    }

    /// Reconnects now, without waiting for the backoff.
    func restart() {
        stop()
        start()
    }

    private func run() async {
        var attempt = 0
        while !Task.isCancelled {
            let task = session.webSocketTask(with: request)
            let taskId = task.taskIdentifier
            // A Connect state carries each request's whole tool input; the default limit is 1 MiB.
            task.maximumMessageSize = Self.maximumMessageSize
            socket = task
            task.resume()
            let pinger = Task { [weak task] in
                // Detects a dead network path, which a pending receive alone would not notice.
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(20))
                    guard let task, !Task.isCancelled else { return }
                    task.sendPing { error in if error != nil { task.cancel() } }
                }
            }
            var certMismatch = false
            do {
                while !Task.isCancelled {
                    let data: Data
                    switch try await task.receive() {
                    case .string(let text):
                        onRawMessage(text)
                        data = Data(text.utf8)
                    case .data(let bytes):
                        if let text = String(data: bytes, encoding: .utf8) {
                            onRawMessage(text)
                        }
                        data = bytes
                    @unknown default:
                        continue
                    }
                    guard let event = try? HostEvent.decode(data) else { continue }
                    if case .hello = event { attempt = 0 }
                    onEvent(event)
                }
            } catch {
                certMismatch = pinnedSession?.consumeMismatch(taskIdentifier: taskId) ?? false
            }
            pinger.cancel()
            task.cancel(with: .goingAway, reason: nil)
            if Task.isCancelled { return }
            onDisconnect(attempt, certMismatch)
            if Task.isCancelled { return }
            let delay = min(30.0, pow(2.0, Double(attempt)))
            attempt += 1
            try? await Task.sleep(for: .seconds(delay))
        }
    }
}
