import Foundation
import WebKit

/// Shared constants and pure security/routing helpers for the `codeck-web://host` Transcript bridge.
enum HostWebBridge {
    static let scheme = "codeck-web"
    static let authority = "host"
    static let transcriptPath = "/web/transcript.html"
    static let rpcHandlerName = "codeckRpc"
    static let syntheticHello = #"{"event":"hello","payload":{"apiVersion":2}}"#

    static let forwardedHeaderNames = [
        "Content-Type",
        "Content-Security-Policy",
        "Cache-Control",
        "X-Content-Type-Options",
    ]

    struct SchemeTarget: Equatable, Sendable {
        var path: String
        var query: String?
    }

    enum NavigationDecision: Equatable, Sendable {
        case allow
        case openExternally(URL)
        case cancel
    }

    static func transcriptURL(sessionId: String) -> URL {
        var c = URLComponents()
        c.scheme = scheme
        c.host = authority
        c.path = transcriptPath
        c.queryItems = [URLQueryItem(name: "sessionId", value: sessionId)]
        return c.url!
    }

    /// Validates a `codeck-web` request: authority `host`, GET method, decoded path under `/web/`
    /// with no `..` path segments (including percent-encoded `%2e%2e`).
    static func resolveSchemeRequest(_ request: URLRequest) -> SchemeTarget? {
        guard (request.httpMethod ?? "GET").uppercased() == "GET",
              let url = request.url,
              let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              components.scheme?.lowercased() == scheme,
              components.host?.lowercased() == authority,
              components.port == nil,
              components.user == nil,
              components.password == nil
        else {
            return nil
        }

        let rawPath = components.percentEncodedPath
        let decodedPath = components.path
        guard decodedPath.hasPrefix("/web/") else { return nil }

        // Check both decoded segments and individually unescaped raw segments for `..`.
        let decodedSegments = decodedPath.split(separator: "/", omittingEmptySubsequences: false)
        if decodedSegments.contains("..") { return nil }

        let rawSegments = rawPath.split(separator: "/", omittingEmptySubsequences: false)
        for seg in rawSegments {
            if seg == ".." { return nil }
            if let unescaped = String(seg).removingPercentEncoding, unescaped == ".." {
                return nil
            }
        }

        return SchemeTarget(path: decodedPath, query: components.percentEncodedQuery)
    }

    /// Message handlers (`codeck` and `codeckRpc`) only trust the main frame of `codeck-web://host`.
    static func isAllowedOrigin(isMainFrame: Bool, protocol proto: String, host: String) -> Bool {
        isMainFrame && proto.lowercased() == scheme && host.lowercased() == authority
    }

    /// Navigation lock: main frame only loads `/web/transcript.html` (external http/https links open in
    /// Safari); subframes only load under `codeck-web://host/web/`.
    static func navigationDecision(
        url: URL?,
        isMainFrame: Bool,
        isLinkActivated: Bool
    ) -> NavigationDecision {
        guard let url, let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            return .cancel
        }
        let scheme = components.scheme?.lowercased()
        let host = components.host?.lowercased()
        if isMainFrame {
            if scheme == Self.scheme,
               host == Self.authority,
               components.port == nil,
               components.user == nil,
               components.password == nil,
               components.path == Self.transcriptPath {
                return .allow
            }
            if isLinkActivated, scheme == "http" || scheme == "https" {
                return .openExternally(url)
            }
            return .cancel
        } else {
            var req = URLRequest(url: url)
            req.httpMethod = "GET"
            return resolveSchemeRequest(req) != nil ? .allow : .cancel
        }
    }

    /// Copies only the allowed response headers from the Host response and sets Content-Length.
    static func forwardedHeaders(from response: HTTPURLResponse, bodyLength: Int) -> [String: String] {
        var headers: [String: String] = ["Content-Length": "\(bodyLength)"]
        for name in forwardedHeaderNames {
            if let value = response.value(forHTTPHeaderField: name) {
                headers[name] = value
            }
        }
        return headers
    }
}

/// Proxies `codeck-web://host/web/*` GET requests through `HostClient` (TLS pinned + Bearer token,
/// no redirects, no caching) and returns responses constructed with the original custom URL.
@MainActor
final class HostSchemeHandler: NSObject, WKURLSchemeHandler {
    typealias Fetcher = @Sendable (_ path: String, _ query: String?) async throws -> (Data, HTTPURLResponse)

    private let currentGeneration: @MainActor () -> Int
    private let makeFetcher: @MainActor () -> Fetcher?
    private var activeTasks: [ObjectIdentifier: Task<Void, Never>] = [:]

    convenience init(model: HostModel) {
        self.init(
            currentGeneration: { [weak model] in model?.generation ?? -1 },
            makeFetcher: { [weak model] in
                guard let client = model?.client else { return nil }
                return { path, query in
                    try await client.get(path: path, query: query)
                }
            }
        )
    }

    init(
        currentGeneration: @escaping @MainActor () -> Int,
        makeFetcher: @escaping @MainActor () -> Fetcher?
    ) {
        self.currentGeneration = currentGeneration
        self.makeFetcher = makeFetcher
        super.init()
    }

    func cancelAll() {
        for task in activeTasks.values { task.cancel() }
        activeTasks.removeAll()
    }

    func webView(_: WKWebView, start urlSchemeTask: any WKURLSchemeTask) {
        guard let originalURL = urlSchemeTask.request.url else {
            urlSchemeTask.didFailWithError(URLError(.badURL))
            return
        }
        guard let target = HostWebBridge.resolveSchemeRequest(urlSchemeTask.request) else {
            respond404(to: urlSchemeTask, url: originalURL)
            return
        }
        guard let fetcher = makeFetcher() else {
            urlSchemeTask.didFailWithError(URLError(.cannotConnectToHost))
            return
        }

        let key = ObjectIdentifier(urlSchemeTask as AnyObject)
        let startGen = currentGeneration()
        let task = Task { [weak self] in
            do {
                let (data, http) = try await fetcher(target.path, target.query)
                guard let self, self.activeTasks.removeValue(forKey: key) != nil else { return }
                guard self.currentGeneration() == startGen else {
                    urlSchemeTask.didFailWithError(URLError(.cancelled))
                    return
                }
                let headers = HostWebBridge.forwardedHeaders(from: http, bodyLength: data.count)
                guard let response = HTTPURLResponse(
                    url: originalURL,
                    statusCode: http.statusCode,
                    httpVersion: "HTTP/1.1",
                    headerFields: headers
                ) else {
                    urlSchemeTask.didFailWithError(URLError(.badServerResponse))
                    return
                }
                urlSchemeTask.didReceive(response)
                urlSchemeTask.didReceive(data)
                urlSchemeTask.didFinish()
            } catch {
                guard let self, self.activeTasks.removeValue(forKey: key) != nil else { return }
                urlSchemeTask.didFailWithError(error)
            }
        }
        activeTasks[key] = task
    }

    func webView(_: WKWebView, stop urlSchemeTask: any WKURLSchemeTask) {
        let key = ObjectIdentifier(urlSchemeTask as AnyObject)
        activeTasks.removeValue(forKey: key)?.cancel()
    }

    private func respond404(to task: any WKURLSchemeTask, url: URL) {
        let body = Data("Not Found".utf8)
        let response = HTTPURLResponse(
            url: url,
            statusCode: 404,
            httpVersion: "HTTP/1.1",
            headerFields: [
                "Content-Type": "text/plain; charset=utf-8",
                "Content-Length": "\(body.count)",
            ]
        )!
        task.didReceive(response)
        task.didReceive(body)
        task.didFinish()
    }
}

/// Handles `window.webkit.messageHandlers.codeckRpc.postMessage({ method, args })` for the Transcript page.
@MainActor
final class HostRPCMessageHandler: NSObject, WKScriptMessageHandlerWithReply {
    typealias Forwarder = @Sendable (_ method: String, _ args: JSONValue) async throws -> Data

    private static let forbiddenReply: [String: Any] = [
        "error": ["code": "forbidden", "message": "不接受这个来源"],
    ]
    private static let httpErrorReply: [String: Any] = [
        "error": ["code": "http", "message": "连不上 Host"],
    ]

    private let currentGeneration: @MainActor () -> Int
    private let makeForwarder: @MainActor () -> Forwarder?

    convenience init(model: HostModel) {
        self.init(
            currentGeneration: { [weak model] in model?.generation ?? -1 },
            makeForwarder: { [weak model] in
                guard let client = model?.client else { return nil }
                return { method, args in
                    try await client.forwardRPC(method: method, args: args)
                }
            }
        )
    }

    init(
        currentGeneration: @escaping @MainActor () -> Int,
        makeForwarder: @escaping @MainActor () -> Forwarder?
    ) {
        self.currentGeneration = currentGeneration
        self.makeForwarder = makeForwarder
        super.init()
    }

    func userContentController(
        _: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void
    ) {
        let frame = message.frameInfo
        guard HostWebBridge.isAllowedOrigin(
            isMainFrame: frame.isMainFrame,
            protocol: frame.securityOrigin.protocol,
            host: frame.securityOrigin.host
        ) else {
            replyHandler(Self.forbiddenReply, nil)
            return
        }

        guard let body = message.body as? [String: Any],
              let method = body["method"] as? String,
              let argsValue = decodeArgs(body["args"])
        else {
            replyHandler(["error": ["code": "invalid_request", "message": "无效的 RPC 请求"]], nil)
            return
        }

        guard let forwarder = makeForwarder() else {
            replyHandler(Self.httpErrorReply, nil)
            return
        }

        let startGen = currentGeneration()
        Task { [weak self] in
            do {
                let data = try await forwarder(method, argsValue)
                guard let self, self.currentGeneration() == startGen else {
                    replyHandler(Self.httpErrorReply, nil)
                    return
                }
                if let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                    replyHandler(obj, nil)
                } else {
                    replyHandler(Self.httpErrorReply, nil)
                }
            } catch {
                replyHandler(Self.httpErrorReply, nil)
            }
        }
    }

    private func decodeArgs(_ raw: Any?) -> JSONValue? {
        if raw == nil || raw is NSNull { return .object([:]) }
        guard let raw, JSONSerialization.isValidJSONObject(raw),
              let data = try? JSONSerialization.data(withJSONObject: raw),
              let value = try? JSONDecoder().decode(JSONValue.self, from: data)
        else {
            return nil
        }
        return value
    }
}

/// Manages pushing raw Host events into a Transcript page after it posts a real `ready` message.
/// Tracks both page reload generation and Host generation so stale pages never receive events.
@MainActor
final class TranscriptEventBridge {
    private let currentHostGeneration: () -> Int
    private let subscribe: (@escaping (String) -> Void) -> () -> Void
    private let push: (String) -> Void

    private var pageGeneration = 0
    private var expectedHostGeneration: Int?
    private var ready = false
    private var unsubscribe: (() -> Void)?

    init(
        currentHostGeneration: @escaping () -> Int,
        subscribe: @escaping (@escaping (String) -> Void) -> () -> Void,
        push: @escaping (String) -> Void
    ) {
        self.currentHostGeneration = currentHostGeneration
        self.subscribe = subscribe
        self.push = push
    }

    /// Resets state for a new page load; stops any previous subscription until the new page posts `ready`.
    func beginLoad(hostGeneration: Int) {
        stop()
        expectedHostGeneration = hostGeneration
    }

    /// Called only on a real `{ type: "ready" }` message from the page (never on the 1.5s display fallback).
    func pageDidReady() {
        guard !ready,
              let hostGen = expectedHostGeneration,
              currentHostGeneration() == hostGen
        else { return }
        ready = true
        let pageGen = pageGeneration
        // Push a synthetic hello first so the page runs its initial resync after mounting subscriptions.
        push(HostWebBridge.syntheticHello)
        unsubscribe = subscribe { [weak self] text in
            guard let self else { return }
            guard self.ready,
                  self.pageGeneration == pageGen,
                  self.currentHostGeneration() == hostGen
            else {
                self.stop()
                return
            }
            self.push(text)
        }
    }

    /// Stops forwarding events and invalidates the current page generation.
    func stop() {
        pageGeneration += 1
        ready = false
        unsubscribe?()
        unsubscribe = nil
    }
}
