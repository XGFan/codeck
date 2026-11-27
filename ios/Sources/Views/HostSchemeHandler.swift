import Foundation
import WebKit

/// Shared constants and pure security/routing helpers for the `codeck-web://host` Transcript bridge.
enum HostWebBridge {
    static let scheme = "codeck-web"
    static let authority = "host"
    static let transcriptPath = "/web/transcript.html"
    static let rpcHandlerName = "codeckRpc"
    static let navQueryItemName = "nav"
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

    /// Appends (or replaces) a per-navigation `nav=<navID>` query item so reloading the same
    /// logical URL produces a distinct document URL for `ready` and message matching.
    static func withNavID(_ url: URL, navID: String = UUID().uuidString) -> URL {
        guard var components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            return url
        }
        var items = (components.queryItems ?? []).filter { $0.name != navQueryItemName }
        items.append(URLQueryItem(name: navQueryItemName, value: navID))
        components.queryItems = items
        return components.url ?? url
    }

    /// Extracts the non-empty `nav` query parameter from a page URL, if present.
    static func navID(of url: URL?) -> String? {
        guard let url,
              let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let value = components.queryItems?.first(where: { $0.name == navQueryItemName })?.value,
              !value.isEmpty
        else { return nil }
        return value
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

        return SchemeTarget(path: rawPath, query: components.percentEncodedQuery)
    }

    /// Message handlers (`codeck` and `codeckRpc`) only trust the main frame of `codeck-web://host`.
    static func isAllowedOrigin(isMainFrame: Bool, protocol proto: String, host: String, port: Int = 0) -> Bool {
        isMainFrame && proto.lowercased() == scheme && host.lowercased() == authority && port == 0
    }

    /// Compares a script message frame URL against the current committed page URL (scheme, host, port,
    /// percent-encoded path and query; ignores fragment).
    static func matchesPageURL(_ candidate: URL?, expected: URL?) -> Bool {
        guard let candidate, let expected,
              let c = URLComponents(url: candidate, resolvingAgainstBaseURL: false),
              let e = URLComponents(url: expected, resolvingAgainstBaseURL: false)
        else { return false }
        return c.scheme?.lowercased() == e.scheme?.lowercased()
            && c.host?.lowercased() == e.host?.lowercased()
            && c.port == e.port
            && c.percentEncodedPath == e.percentEncodedPath
            && c.percentEncodedQuery == e.percentEncodedQuery
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

    /// Copies only the allowed response headers from the Host response, sets Content-Length,
    /// and unconditionally forces `Cache-Control: no-store` to prevent WebKit caching.
    static func forwardedHeaders(from response: HTTPURLResponse, bodyLength: Int) -> [String: String] {
        var headers: [String: String] = ["Content-Length": "\(bodyLength)"]
        for name in forwardedHeaderNames {
            if let value = response.value(forHTTPHeaderField: name) {
                headers[name] = value
            }
        }
        headers["Cache-Control"] = "no-store"
        return headers
    }
}

/// Proxies `codeck-web://host/web/*` GET requests through the creation-time `HostClient` (TLS pinned
/// + Bearer token, no redirects, no caching) and returns responses constructed with the original custom URL.
/// Bound to a single Host generation: once the Host changes, every entry and in-flight callback returns 404.
@MainActor
final class HostSchemeHandler: NSObject, WKURLSchemeHandler {
    typealias Fetcher = @Sendable (_ path: String, _ query: String?) async throws -> (Data, HTTPURLResponse)

    let boundGeneration: Int
    private let currentGeneration: @MainActor () -> Int
    private let fetcher: Fetcher?
    private let onUnauthorized: @MainActor () -> Void
    private var activeTasks: [ObjectIdentifier: Task<Void, Never>] = [:]

    convenience init(model: HostModel) {
        let boundGen = model.generation
        let boundClient = model.client
        let fetcher: Fetcher? = boundClient.map { client in
            { @Sendable path, query in
                try await client.get(path: path, query: query)
            }
        }
        self.init(
            boundGeneration: boundGen,
            currentGeneration: { [weak model] in model?.generation ?? -1 },
            fetcher: fetcher,
            onUnauthorized: { [weak model] in
                model?.handleBridgeUnauthorized(generation: boundGen)
            }
        )
    }

    init(
        boundGeneration: Int,
        currentGeneration: @escaping @MainActor () -> Int,
        fetcher: Fetcher?,
        onUnauthorized: @escaping @MainActor () -> Void = {}
    ) {
        self.boundGeneration = boundGeneration
        self.currentGeneration = currentGeneration
        self.fetcher = fetcher
        self.onUnauthorized = onUnauthorized
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
        guard currentGeneration() == boundGeneration else {
            respond404(to: urlSchemeTask, url: originalURL)
            return
        }
        guard let target = HostWebBridge.resolveSchemeRequest(urlSchemeTask.request) else {
            respond404(to: urlSchemeTask, url: originalURL)
            return
        }
        guard let fetcher else {
            urlSchemeTask.didFailWithError(URLError(.cannotConnectToHost))
            return
        }

        let key = ObjectIdentifier(urlSchemeTask as AnyObject)
        let task = Task { [weak self] in
            do {
                let (data, http) = try await fetcher(target.path, target.query)
                guard let self, self.activeTasks.removeValue(forKey: key) != nil else { return }
                guard self.currentGeneration() == self.boundGeneration else {
                    self.respond404(to: urlSchemeTask, url: originalURL)
                    return
                }
                if http.statusCode == 401 {
                    self.onUnauthorized()
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
                if self.currentGeneration() != self.boundGeneration {
                    self.respond404(to: urlSchemeTask, url: originalURL)
                } else {
                    urlSchemeTask.didFailWithError(error)
                }
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
                "Cache-Control": "no-store",
            ]
        )!
        task.didReceive(response)
        task.didReceive(body)
        task.didFinish()
    }
}

/// Handles `window.webkit.messageHandlers.codeckRpc.postMessage({ method, args })` for the Transcript page.
/// Bound to the creation-time Host generation and client: rejects requests once the Host changes.
@MainActor
final class HostRPCMessageHandler: NSObject, WKScriptMessageHandlerWithReply {
    typealias Forwarder = @Sendable (_ method: String, _ args: JSONValue) async throws -> (Data, HTTPURLResponse)

    private static let forbiddenReply: [String: Any] = [
        "error": ["code": "forbidden", "message": "不接受这个来源"],
    ]
    private static let hostChangedReply: [String: Any] = [
        "error": ["code": "forbidden", "message": "Host 已更换"],
    ]
    private static let httpErrorReply: [String: Any] = [
        "error": ["code": "http", "message": "连不上 Host"],
    ]

    let boundGeneration: Int
    weak var expectedWebView: WKWebView?
    private let currentGeneration: @MainActor () -> Int
    private let forwarder: Forwarder?
    private let onUnauthorized: @MainActor () -> Void

    convenience init(model: HostModel) {
        let boundGen = model.generation
        let boundClient = model.client
        let forwarder: Forwarder? = boundClient.map { client in
            { @Sendable method, args in
                try await client.forwardRPC(method: method, args: args)
            }
        }
        self.init(
            boundGeneration: boundGen,
            currentGeneration: { [weak model] in model?.generation ?? -1 },
            forwarder: forwarder,
            onUnauthorized: { [weak model] in
                model?.handleBridgeUnauthorized(generation: boundGen)
            }
        )
    }

    init(
        boundGeneration: Int,
        currentGeneration: @escaping @MainActor () -> Int,
        forwarder: Forwarder?,
        onUnauthorized: @escaping @MainActor () -> Void = {}
    ) {
        self.boundGeneration = boundGeneration
        self.currentGeneration = currentGeneration
        self.forwarder = forwarder
        self.onUnauthorized = onUnauthorized
        super.init()
    }

    func userContentController(
        _: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void
    ) {
        if let expectedWebView, message.webView !== expectedWebView {
            replyHandler(Self.forbiddenReply, nil)
            return
        }
        let frame = message.frameInfo
        guard HostWebBridge.isAllowedOrigin(
            isMainFrame: frame.isMainFrame,
            protocol: frame.securityOrigin.protocol,
            host: frame.securityOrigin.host,
            port: frame.securityOrigin.port
        ) else {
            replyHandler(Self.forbiddenReply, nil)
            return
        }

        guard currentGeneration() == boundGeneration else {
            replyHandler(Self.hostChangedReply, nil)
            return
        }

        guard let body = message.body as? [String: Any],
              let method = body["method"] as? String,
              let argsValue = decodeArgs(body["args"])
        else {
            replyHandler(["error": ["code": "invalid_request", "message": "无效的 RPC 请求"]], nil)
            return
        }

        guard let forwarder else {
            replyHandler(Self.httpErrorReply, nil)
            return
        }

        Task { [weak self] in
            do {
                let (data, http) = try await forwarder(method, argsValue)
                guard let self, self.currentGeneration() == self.boundGeneration else {
                    replyHandler(Self.hostChangedReply, nil)
                    return
                }
                if http.statusCode == 401 {
                    self.onUnauthorized()
                }
                if let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                    replyHandler(obj, nil)
                } else {
                    replyHandler(Self.httpErrorReply, nil)
                }
            } catch {
                guard let self, self.currentGeneration() == self.boundGeneration else {
                    replyHandler(Self.hostChangedReply, nil)
                    return
                }
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

/// Manages pushing raw Host events into a Transcript page after `didCommit` and a matching `ready` message.
/// Bound to a single Host generation and tracks committed page URL + reload generation so stale pages
/// never receive events.
@MainActor
final class TranscriptEventBridge {
    let boundHostGeneration: Int
    private let currentHostGeneration: () -> Int
    private let subscribe: (@escaping (String) -> Void) -> () -> Void
    private let push: (String) -> Void

    private var pageGeneration = 0
    private var expectedNavID: String?
    private var committedURL: URL?
    private var ready = false
    private var unsubscribe: (() -> Void)?

    init(
        boundHostGeneration: Int,
        currentHostGeneration: @escaping () -> Int,
        subscribe: @escaping (@escaping (String) -> Void) -> () -> Void,
        push: @escaping (String) -> Void
    ) {
        self.boundHostGeneration = boundHostGeneration
        self.currentHostGeneration = currentHostGeneration
        self.subscribe = subscribe
        self.push = push
    }

    /// Stops forwarding immediately when a new provisional navigation begins and clears the committed page.
    /// When `expectedNavID` is supplied, only commits and `ready` messages carrying that exact `nav` pass.
    func didStartNavigation(expectedNavID: String? = nil) {
        stop()
        committedURL = nil
        self.expectedNavID = expectedNavID
    }

    /// Marks the current main-frame navigation as committed for `url` if the Host generation and expected `nav` match.
    func didCommit(url: URL?) {
        stop()
        guard currentHostGeneration() == boundHostGeneration else {
            committedURL = nil
            return
        }
        if let expectedNavID, HostWebBridge.navID(of: url) != expectedNavID {
            committedURL = nil
            return
        }
        committedURL = url
    }

    /// Called only on a real `{ type: "ready" }` message from the page (never on the 1.5s display fallback).
    /// Accepted only after `didCommit`, when `frameURL` matches the committed page (and `nav` id) and Host generation matches.
    @discardableResult
    func pageDidReady(frameURL: URL?) -> Bool {
        guard !ready,
              let committedURL,
              currentHostGeneration() == boundHostGeneration,
              HostWebBridge.matchesPageURL(frameURL, expected: committedURL)
        else { return false }
        if let expectedNavID, HostWebBridge.navID(of: frameURL) != expectedNavID {
            return false
        }
        ready = true
        let pageGen = pageGeneration
        // Push a synthetic hello first so the page runs its initial resync after mounting subscriptions.
        push(HostWebBridge.syntheticHello)
        unsubscribe = subscribe { [weak self] text in
            guard let self else { return }
            guard self.ready,
                  self.pageGeneration == pageGen,
                  self.currentHostGeneration() == self.boundHostGeneration
            else {
                self.stop()
                return
            }
            self.push(text)
        }
        return true
    }

    /// Stops forwarding events and invalidates the current page generation.
    func stop() {
        pageGeneration += 1
        ready = false
        unsubscribe?()
        unsubscribe = nil
    }
}
