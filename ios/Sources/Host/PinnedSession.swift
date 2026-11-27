import CryptoKit
import Foundation
import Security

enum CertificatePinning {
    /// Unpadded base64url SHA-256 digest of a leaf certificate's DER bytes (43 characters).
    static func fingerprint(of der: Data) -> String {
        let digest = SHA256.hash(data: der)
        return Data(digest)
            .base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

/// Task-level URLSession delegate that pins the leaf certificate SHA-256 fingerprint,
/// rejects HTTP redirects, tracks certificate mismatches per task identifier, and supports
/// streaming byte-bounded reads for unauthenticated `/health` probes.
///
/// Intentionally does NOT implement the session-level `urlSession(_:didReceive:completionHandler:)`
/// so server-trust challenges for both data tasks and WebSocket tasks arrive here with their `task`.
final class PinnedSessionDelegate: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    let expectedFingerprint: String?

    private final class BoundedTaskState {
        let maxBytes: Int
        let continuation: CheckedContinuation<(Data, URLResponse), Error>
        var response: URLResponse?
        var data = Data()
        var exceededLimit = false

        init(maxBytes: Int, continuation: CheckedContinuation<(Data, URLResponse), Error>) {
            self.maxBytes = maxBytes
            self.continuation = continuation
        }
    }

    private let lock = NSLock()
    private var mismatchedTasks: Set<Int> = []
    private var boundedTasks: [Int: BoundedTaskState] = [:]

    init(expectedFingerprint: String?) {
        self.expectedFingerprint = expectedFingerprint
    }

    func recordMismatch(taskIdentifier: Int) {
        lock.withLock { _ = mismatchedTasks.insert(taskIdentifier) }
    }

    func consumeMismatch(taskIdentifier: Int) -> Bool {
        lock.withLock { mismatchedTasks.remove(taskIdentifier) != nil }
    }

    fileprivate func registerBoundedTask(
        taskIdentifier: Int,
        maxBytes: Int,
        continuation: CheckedContinuation<(Data, URLResponse), Error>
    ) {
        lock.withLock {
            boundedTasks[taskIdentifier] = BoundedTaskState(maxBytes: maxBytes, continuation: continuation)
        }
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust else {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        guard let expected = expectedFingerprint,
              let trust = challenge.protectionSpace.serverTrust,
              let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
              let leaf = chain.first
        else {
            recordMismatch(taskIdentifier: task.taskIdentifier)
            completionHandler(.cancelAuthenticationChallenge, nil)
            return
        }
        let der = SecCertificateCopyData(leaf) as Data
        if CertificatePinning.fingerprint(of: der) == expected {
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            recordMismatch(taskIdentifier: task.taskIdentifier)
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping @Sendable (URLRequest?) -> Void
    ) {
        // Never follow redirects from the Host (prevents credential leakage or scheme downgrade).
        completionHandler(nil)
    }

    func urlSession(
        _ session: URLSession,
        dataTask: URLSessionDataTask,
        didReceive response: URLResponse,
        completionHandler: @escaping @Sendable (URLSession.ResponseDisposition) -> Void
    ) {
        let disposition: URLSession.ResponseDisposition = lock.withLock {
            guard let state = boundedTasks[dataTask.taskIdentifier] else {
                return .allow
            }
            state.response = response
            if response.expectedContentLength > Int64(state.maxBytes) {
                state.exceededLimit = true
                state.data.removeAll(keepingCapacity: false)
                return .cancel
            }
            return .allow
        }
        completionHandler(disposition)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        let shouldCancel: Bool = lock.withLock {
            guard let state = boundedTasks[dataTask.taskIdentifier], !state.exceededLimit else {
                return false
            }
            if state.data.count + data.count > state.maxBytes {
                state.exceededLimit = true
                state.data.removeAll(keepingCapacity: false)
                return true
            }
            state.data.append(data)
            return false
        }
        if shouldCancel {
            dataTask.cancel()
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: (any Error)?) {
        let (state, mismatched): (BoundedTaskState?, Bool) = lock.withLock {
            let state = boundedTasks.removeValue(forKey: task.taskIdentifier)
            // Only consume mismatch flags for bounded tasks handled here; completion-handler and WebSocket
            // tasks consume their own mismatch flags in their callers.
            let mismatched = state != nil ? (mismatchedTasks.remove(task.taskIdentifier) != nil) : false
            return (state, mismatched)
        }
        guard let state else { return }
        if mismatched {
            state.continuation.resume(throwing: HostError.certificateChanged)
        } else if state.exceededLimit {
            state.continuation.resume(throwing: HostError.badResponse)
        } else if let error {
            state.continuation.resume(throwing: error)
        } else if let response = state.response {
            state.continuation.resume(returning: (state.data, response))
        } else {
            state.continuation.resume(throwing: HostError.badResponse)
        }
    }
}

/// Owns a `URLSession` configured with `PinnedSessionDelegate` and `urlCache = nil`,
/// invalidating the session on deinit so URLSession does not retain its delegate forever.
final class PinnedSession: Sendable {
    let delegate: PinnedSessionDelegate
    let session: URLSession

    init(expectedFingerprint: String?, protocolClasses: [AnyClass]? = nil) {
        let delegate = PinnedSessionDelegate(expectedFingerprint: expectedFingerprint)
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.httpCookieStorage = nil
        config.httpShouldSetCookies = false
        if let protocolClasses {
            config.protocolClasses = protocolClasses
        }
        self.delegate = delegate
        self.session = URLSession(configuration: config, delegate: delegate, delegateQueue: nil)
    }

    deinit {
        session.finishTasksAndInvalidate()
    }

    func consumeMismatch(taskIdentifier: Int) -> Bool {
        delegate.consumeMismatch(taskIdentifier: taskIdentifier)
    }

    /// Performs a data task and turns per-task certificate mismatches into `HostError.certificateChanged`.
    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        let holder = DataTaskHolder()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                let task = session.dataTask(with: request) { [delegate] data, response, error in
                    let mismatched = holder.taskIdentifier.map { delegate.consumeMismatch(taskIdentifier: $0) } ?? false
                    if mismatched {
                        continuation.resume(throwing: HostError.certificateChanged)
                        return
                    }
                    if let error {
                        continuation.resume(throwing: error)
                        return
                    }
                    guard let data, let response else {
                        continuation.resume(throwing: HostError.badResponse)
                        return
                    }
                    continuation.resume(returning: (data, response))
                }
                holder.set(task)
                task.resume()
            }
        } onCancel: {
            holder.cancel()
        }
    }

    /// Streams a data task through `URLSessionDataDelegate` and cancels immediately if the response
    /// declares or delivers more than `maxBytes`, without buffering an unbounded body.
    func data(for request: URLRequest, maxBytes: Int) async throws -> (Data, URLResponse) {
        let holder = DataTaskHolder()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                let task = session.dataTask(with: request)
                delegate.registerBoundedTask(
                    taskIdentifier: task.taskIdentifier,
                    maxBytes: maxBytes,
                    continuation: continuation
                )
                holder.set(task)
                task.resume()
            }
        } onCancel: {
            holder.cancel()
        }
    }
}

private final class DataTaskHolder: @unchecked Sendable {
    private let lock = NSLock()
    private var task: URLSessionDataTask?
    private var cancelled = false

    func set(_ task: URLSessionDataTask) {
        let shouldCancel = lock.withLock {
            self.task = task
            return cancelled
        }
        if shouldCancel { task.cancel() }
    }

    func cancel() {
        let current = lock.withLock { () -> URLSessionDataTask? in
            cancelled = true
            return task
        }
        current?.cancel()
    }

    var taskIdentifier: Int? {
        lock.withLock { task?.taskIdentifier }
    }
}
