import SwiftUI
import WebKit

/// What the page's 「回退」 and 「Fork」 buttons show (`window.codeckPage.setConnect`, state/native.ts): the app holds the
/// Connect, the page only draws the buttons and hands a tap back.
struct PageConnect: Encodable, Equatable {
    struct Rewound: Encodable, Equatable {
        var uuid: String
        var withCode: Bool
    }

    /// A Connect that takes commands drives the Session: its Main Line prompts offer 「回退」.
    var connected = false
    /// Idle, so a rewind can start.
    var idle = false
    /// The prompt whose confirm page a `rewind` is opening.
    var rewinding: String?
    var rewound: Rewound?
    var canFork = false
    var forking = false
    /// Code Mode: the calls the running code made that the transcript does not have yet (the Host folds them).
    var codeCalls: [JSONValue] = []
    /// Working: the page shows the progress line at the end of the conversation, as on the desktop.
    var busy = false
    /// What it is doing (Claude Code's spinner line), while busy.
    var progress: String?

    /// `c`: the Session's Connect while it takes commands.
    static func of(_ c: ConnState?, rewinding: String?, rewound: CoDeck.Rewound?, fork: StartOffer, forking: Bool) -> PageConnect {
        PageConnect(
            connected: c != nil,
            idle: c?.state == .idle,
            rewinding: rewinding,
            rewound: rewound.map { Rewound(uuid: $0.uuid, withCode: $0.withCode) },
            canFork: fork == .enabled,
            forking: forking,
            codeCalls: c?.codeCalls ?? [],
            busy: c?.state == .busy,
            progress: c?.state == .busy ? c?.progress.flatMap { $0.text ?? $0.verb } : nil
        )
    }
}

/// The Host's read-only Transcript page (`codeck-web://host/web/transcript.html?sessionId=…`) in a WKWebView.
/// Static assets, RPC, and events are all relayed through the app (`HostSchemeHandler`, `codeckRpc`, and
/// `window.codeckPage.event`) so the page never talks to the Host directly.
struct TranscriptWebView: UIViewRepresentable {
    let url: URL
    let reloadToken: Int
    let model: HostModel
    var onReady: () -> Void
    var onFailure: (String) -> Void
    var onCopy: (String) -> Void
    var bottomInset: CGFloat
    var topInset: CGFloat
    var connect = PageConnect()
    /// 「回退」 was tapped under the prompt with this uuid.
    var onRewind: (String) -> Void = { _ in }
    /// 「Fork」 was tapped under the prompt with this uuid.
    var onFork: (String) -> Void = { _ in }
    /// The page was tapped or dragged.
    var onTouch: () -> Void = {}

    static let handlerName = "codeck"

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        // Isolate WebKit cache/storage per WebView/Host generation.
        config.websiteDataStore = .nonPersistent()
        config.setURLSchemeHandler(context.coordinator.schemeHandler, forURLScheme: HostWebBridge.scheme)
        config.userContentController.add(WeakHandler(context.coordinator), contentWorld: .page, name: Self.handlerName)
        config.userContentController.addScriptMessageHandler(
            WeakReplyHandler(context.coordinator.rpcHandler),
            contentWorld: .page,
            name: HostWebBridge.rpcHandlerName
        )
        config.allowsInlineMediaPlayback = true
        let web = WKWebView(frame: .zero, configuration: config)
        web.isOpaque = false
        web.backgroundColor = .clear
        web.scrollView.backgroundColor = .clear
        web.scrollView.contentInsetAdjustmentBehavior = .never
        web.allowsBackForwardNavigationGestures = false
        web.navigationDelegate = context.coordinator
        context.coordinator.attach(web)
        // Beside the page's own handling of the touch: the app puts its keyboard away.
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.touched))
        let pan = UIPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.touched))
        for g in [tap, pan] as [UIGestureRecognizer] {
            g.cancelsTouchesInView = false
            g.delegate = context.coordinator
            web.addGestureRecognizer(g)
        }
        #if DEBUG
        web.isInspectable = true
        #endif
        context.coordinator.load(web, url: url, token: reloadToken)
        return web
    }

    func updateUIView(_ web: WKWebView, context: Context) {
        context.coordinator.parent = self
        guard context.coordinator.boundGeneration == model.generation else {
            // Host changed: revoke this WebView; SessionDetailView recreates a fresh one via `.id(model.generation)`.
            context.coordinator.teardown()
            return
        }
        if context.coordinator.loadedURL != url || context.coordinator.loadedToken != reloadToken {
            context.coordinator.load(web, url: url, token: reloadToken)
        }
        context.coordinator.writeInsets(web)
        context.coordinator.writeConnect(web)
    }

    static func dismantleUIView(_ web: WKWebView, coordinator: Coordinator) {
        web.configuration.userContentController.removeScriptMessageHandler(forName: handlerName, contentWorld: .page)
        web.configuration.userContentController.removeScriptMessageHandler(
            forName: HostWebBridge.rpcHandlerName,
            contentWorld: .page
        )
        coordinator.teardown()
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler, UIGestureRecognizerDelegate {
        var parent: TranscriptWebView
        let boundGeneration: Int
        var loadedURL: URL?
        var loadedToken = 0
        var readyFallback: Task<Void, Never>?
        let schemeHandler: HostSchemeHandler
        let rpcHandler: HostRPCMessageHandler
        let eventBridge: TranscriptEventBridge

        private weak var webView: WKWebView?
        private var activeNavigation: WKNavigation?
        private var startingLoad = false
        private var pendingNavID: String?
        private var pendingNavURL: URL?
        private var committedURL: URL?
        private var ready = false
        private var writtenInsets: [CGFloat]?
        private var writtenConnect: PageConnect?

        init(parent: TranscriptWebView) {
            let model = parent.model
            let boundGen = model.generation
            self.parent = parent
            self.boundGeneration = boundGen
            self.schemeHandler = HostSchemeHandler(model: model)
            self.rpcHandler = HostRPCMessageHandler(model: model)
            var pushRef: ((String) -> Void)?
            self.eventBridge = TranscriptEventBridge(
                boundHostGeneration: boundGen,
                currentHostGeneration: { [weak model] in model?.generation ?? -1 },
                subscribe: { [weak model] handler in
                    guard let model, model.generation == boundGen else { return {} }
                    return model.subscribeRawEvents(handler)
                },
                push: { text in
                    pushRef?(text)
                }
            )
            super.init()
            pushRef = { [weak self] text in
                guard let self, self.parent.model.generation == self.boundGeneration else { return }
                self.webView?.callAsyncJavaScript(
                    "window.codeckPage?.event?.(e)",
                    arguments: ["e": text],
                    in: nil,
                    in: .page,
                    completionHandler: nil
                )
            }
        }

        func attach(_ web: WKWebView) {
            self.webView = web
            self.rpcHandler.expectedWebView = web
        }

        func teardown() {
            readyFallback?.cancel()
            readyFallback = nil
            pendingNavID = nil
            pendingNavURL = nil
            committedURL = nil
            activeNavigation = nil
            schemeHandler.cancelAll()
            eventBridge.didStartNavigation()
        }

        func load(_ web: WKWebView, url: URL, token: Int) {
            guard parent.model.generation == boundGeneration else {
                teardown()
                return
            }
            loadedURL = url
            loadedToken = token
            ready = false
            committedURL = nil
            writtenInsets = nil
            writtenConnect = nil
            readyFallback?.cancel()
            // Stamp each load with a unique `nav` query param so a late `ready` from a previous
            // document of the same logical URL cannot preempt the newly committed reload.
            let navID = UUID().uuidString
            let navURL = HostWebBridge.withNavID(url, navID: navID)
            pendingNavID = navID
            pendingNavURL = navURL
            activeNavigation = nil
            eventBridge.didStartNavigation(expectedNavID: navID)
            startingLoad = true
            let nav = web.load(URLRequest(url: navURL, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 20))
            startingLoad = false
            if activeNavigation == nil {
                activeNavigation = nav
            }
        }

        /// Sets the page's two CSS variables when they changed; a new page starts without them (`force`).
        func writeInsets(_ web: WKWebView, force: Bool = false) {
            guard parent.model.generation == boundGeneration else { return }
            let insets = [parent.bottomInset, parent.topInset].map { $0.rounded() }
            guard force || insets != writtenInsets else { return }
            writtenInsets = insets
            let js = """
            (s => { s.setProperty("--connect-h", "\(Int(insets[0]))px"); s.setProperty("--transcript-top", "\(Int(insets[1]))px"); })(document.documentElement.style)
            """
            web.evaluateJavaScript(js, completionHandler: nil)
        }

        /// Tells the page what its 「回退」 and 「Fork」 buttons show, when that changed; a page takes it only once it
        /// said `ready` (it has `window.codeckPage` from then on), which writes it again (`force`).
        func writeConnect(_ web: WKWebView, force: Bool = false) {
            guard parent.model.generation == boundGeneration else { return }
            guard force || parent.connect != writtenConnect, let json = try? JSONEncoder().encode(parent.connect) else { return }
            writtenConnect = parent.connect
            web.evaluateJavaScript("window.codeckPage?.setConnect(\(String(decoding: json, as: UTF8.self)))", completionHandler: nil)
        }

        @objc func touched(_ g: UIGestureRecognizer) {
            if g.state == .ended || g.state == .began { parent.onTouch() }
        }

        nonisolated func gestureRecognizer(_: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith _: UIGestureRecognizer) -> Bool {
            true
        }

        private func markReady() {
            guard parent.model.generation == boundGeneration, !ready else { return }
            ready = true
            readyFallback?.cancel()
            parent.onReady()
        }

        // MARK: WKScriptMessageHandler

        func userContentController(_: WKUserContentController, didReceive message: WKScriptMessage) {
            if let webView, message.webView !== webView { return }
            guard parent.model.generation == boundGeneration else { return }
            let frame = message.frameInfo
            guard HostWebBridge.isAllowedOrigin(
                isMainFrame: frame.isMainFrame,
                protocol: frame.securityOrigin.protocol,
                host: frame.securityOrigin.host,
                port: frame.securityOrigin.port
            ) else { return }

            guard let body = message.body as? [String: Any], let type = body["type"] as? String else { return }
            switch type {
            case "ready":
                // Only accept `ready` after `didCommit` for the current committed page URL.
                guard eventBridge.pageDidReady(frameURL: frame.request.url) else { return }
                if let web = message.webView {
                    writeInsets(web, force: true)
                    writeConnect(web, force: true)
                }
                markReady()
            case "rewind":
                guard HostWebBridge.matchesPageURL(frame.request.url, expected: committedURL) else { return }
                if let id = body["nodeId"] as? String { parent.onRewind(id) }
            case "fork":
                guard HostWebBridge.matchesPageURL(frame.request.url, expected: committedURL) else { return }
                if let id = body["nodeId"] as? String { parent.onFork(id) }
            case "copyText":
                guard HostWebBridge.matchesPageURL(frame.request.url, expected: committedURL) else { return }
                if let text = body["text"] as? String { parent.onCopy(text) }
            default: break
            }
        }

        // MARK: WKNavigationDelegate

        func webView(_: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
            ready = false
            committedURL = nil
            readyFallback?.cancel()
            if startingLoad {
                activeNavigation = navigation
            } else if activeNavigation !== navigation {
                pendingNavID = nil
                pendingNavURL = nil
                activeNavigation = navigation
            }
            eventBridge.didStartNavigation(expectedNavID: pendingNavID)
        }

        func webView(_ web: WKWebView, didCommit navigation: WKNavigation!) {
            guard parent.model.generation == boundGeneration else {
                teardown()
                return
            }
            guard activeNavigation == nil || activeNavigation === navigation else { return }
            let url = web.url ?? pendingNavURL ?? loadedURL
            if let pendingNavID, HostWebBridge.navID(of: url) != pendingNavID {
                committedURL = nil
                return
            }
            committedURL = url
            eventBridge.didCommit(url: url)
        }

        func webView(_ web: WKWebView, didFinish navigation: WKNavigation!) {
            guard parent.model.generation == boundGeneration else { return }
            guard activeNavigation == nil || activeNavigation === navigation else { return }
            writeInsets(web, force: true)
            // A page that never posts `ready` is shown shortly after it loads, but does NOT start event pushing.
            readyFallback?.cancel()
            readyFallback = Task { [weak self] in
                try? await Task.sleep(for: .seconds(1.5))
                guard !Task.isCancelled else { return }
                self?.markReady()
            }
        }

        func webView(_: WKWebView, didFail _: WKNavigation!, withError error: Error) {
            fail(error)
        }

        func webView(_: WKWebView, didFailProvisionalNavigation _: WKNavigation!, withError error: Error) {
            fail(error)
        }

        private func fail(_ error: Error) {
            if (error as NSError).code == NSURLErrorCancelled { return }
            readyFallback?.cancel()
            eventBridge.didStartNavigation()
            committedURL = nil
            guard parent.model.generation == boundGeneration else { return }
            parent.onFailure("Transcript 加载失败：\(error.localizedDescription)")
        }

        func webView(
            _: WKWebView,
            decidePolicyFor response: WKNavigationResponse
        ) async -> WKNavigationResponsePolicy {
            guard parent.model.generation == boundGeneration else { return .cancel }
            if response.isForMainFrame, let http = response.response as? HTTPURLResponse, http.statusCode >= 400 {
                eventBridge.didStartNavigation()
                committedURL = nil
                parent.onFailure("Host 没有返回 Transcript 页面（HTTP \(http.statusCode)）。")
                return .cancel
            }
            return .allow
        }

        func webView(
            _: WKWebView,
            decidePolicyFor action: WKNavigationAction
        ) async -> WKNavigationActionPolicy {
            guard parent.model.generation == boundGeneration else { return .cancel }
            let decision = HostWebBridge.navigationDecision(
                url: action.request.url,
                isMainFrame: action.targetFrame?.isMainFrame ?? true,
                isLinkActivated: action.navigationType == .linkActivated
            )
            switch decision {
            case .allow:
                return .allow
            case .openExternally(let url):
                await UIApplication.shared.open(url)
                return .cancel
            case .cancel:
                return .cancel
            }
        }

        func webViewWebContentProcessDidTerminate(_ web: WKWebView) {
            guard parent.model.generation == boundGeneration else { return }
            if let loadedURL { load(web, url: loadedURL, token: loadedToken) }
        }
    }
}

/// WKUserContentController retains its handlers; these keep it from retaining the coordinator.
private final class WeakHandler: NSObject, WKScriptMessageHandler {
    weak var target: (any WKScriptMessageHandler)?

    init(_ target: any WKScriptMessageHandler) { self.target = target }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        target?.userContentController(controller, didReceive: message)
    }
}

private final class WeakReplyHandler: NSObject, WKScriptMessageHandlerWithReply {
    weak var target: (any WKScriptMessageHandlerWithReply)?

    init(_ target: any WKScriptMessageHandlerWithReply) { self.target = target }

    func userContentController(
        _ controller: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void
    ) {
        if let target {
            target.userContentController(controller, didReceive: message, replyHandler: replyHandler)
        } else {
            replyHandler(["error": ["code": "http", "message": "连不上 Host"]], nil)
        }
    }
}
