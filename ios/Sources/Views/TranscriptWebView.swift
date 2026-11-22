import SwiftUI
import WebKit

/// What the page's 「回退」 and 「Fork」 buttons show (`window.codeckPage.setConnect`, native.ts): the app holds the
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

/// The Host's read-only Transcript page (`/web/transcript.html?sessionId=…`) in a WKWebView.
/// Bumping `reloadToken` reloads it. It runs under the native floats: `bottomInset` and `topInset` tell the page how
/// much of it they cover.
struct TranscriptWebView: UIViewRepresentable {
    let url: URL
    let reloadToken: Int
    var onReady: () -> Void
    var onFailure: (String) -> Void
    var onCopy: (String) -> Void
    var bottomInset: CGFloat
    var topInset: CGFloat
    var onAtBottom: (Bool) -> Void
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
        config.userContentController.add(WeakHandler(context.coordinator), name: Self.handlerName)
        config.allowsInlineMediaPlayback = true
        let web = WKWebView(frame: .zero, configuration: config)
        web.isOpaque = false
        web.backgroundColor = .clear
        web.scrollView.backgroundColor = .clear
        web.scrollView.contentInsetAdjustmentBehavior = .never
        web.allowsBackForwardNavigationGestures = false
        web.navigationDelegate = context.coordinator
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
        if context.coordinator.loadedURL != url || context.coordinator.loadedToken != reloadToken {
            context.coordinator.load(web, url: url, token: reloadToken)
        }
        context.coordinator.writeInsets(web)
        context.coordinator.writeConnect(web)
    }

    static func dismantleUIView(_ web: WKWebView, coordinator: Coordinator) {
        web.configuration.userContentController.removeScriptMessageHandler(forName: handlerName)
        coordinator.readyFallback?.cancel()
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler, UIGestureRecognizerDelegate {
        var parent: TranscriptWebView
        var loadedURL: URL?
        var loadedToken = 0
        var readyFallback: Task<Void, Never>?
        private var ready = false
        private var writtenInsets: [CGFloat]?
        private var writtenConnect: PageConnect?

        init(parent: TranscriptWebView) { self.parent = parent }

        func load(_ web: WKWebView, url: URL, token: Int) {
            loadedURL = url
            loadedToken = token
            ready = false
            writtenInsets = nil
            writtenConnect = nil
            readyFallback?.cancel()
            // A load may come from inside a view update: not a place to change state.
            Task { [weak self] in self?.parent.onAtBottom(true) }
            web.load(URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 20))
        }

        /// Sets the page's two CSS variables when they changed; a new page starts without them (`force`).
        func writeInsets(_ web: WKWebView, force: Bool = false) {
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
            guard !ready else { return }
            ready = true
            readyFallback?.cancel()
            parent.onReady()
        }

        // MARK: WKScriptMessageHandler

        func userContentController(_: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let body = message.body as? [String: Any], let type = body["type"] as? String else { return }
            switch type {
            case "ready":
                if let web = message.webView {
                    writeInsets(web, force: true)
                    writeConnect(web, force: true)
                }
                markReady()
            case "rewind": if let id = body["nodeId"] as? String { parent.onRewind(id) }
            case "fork": if let id = body["nodeId"] as? String { parent.onFork(id) }
            case "copyText": if let text = body["text"] as? String { parent.onCopy(text) }
            case "scroll": if let atBottom = body["atBottom"] as? Bool { parent.onAtBottom(atBottom) }
            default: break
            }
        }

        // MARK: WKNavigationDelegate

        func webView(_ web: WKWebView, didFinish _: WKNavigation!) {
            writeInsets(web, force: true)
            // A page that never posts `ready` is shown shortly after it loads.
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
            parent.onFailure("Transcript 加载失败：\(error.localizedDescription)")
        }

        func webView(
            _: WKWebView,
            decidePolicyFor response: WKNavigationResponse
        ) async -> WKNavigationResponsePolicy {
            if response.isForMainFrame, let http = response.response as? HTTPURLResponse, http.statusCode >= 400 {
                parent.onFailure("Host 没有返回 Transcript 页面（HTTP \(http.statusCode)）。")
                return .cancel
            }
            return .allow
        }

        func webView(
            _: WKWebView,
            decidePolicyFor action: WKNavigationAction
        ) async -> WKNavigationActionPolicy {
            // Links leaving the Host open in Safari.
            guard let url = action.request.url, action.navigationType == .linkActivated,
                  url.host() != loadedURL?.host() || url.port != loadedURL?.port
            else { return .allow }
            if ["http", "https"].contains(url.scheme) { await UIApplication.shared.open(url) }
            return .cancel
        }

        func webViewWebContentProcessDidTerminate(_ web: WKWebView) {
            if let loadedURL { load(web, url: loadedURL, token: loadedToken) }
        }
    }
}

/// WKUserContentController retains its handlers; this keeps it from retaining the coordinator.
private final class WeakHandler: NSObject, WKScriptMessageHandler {
    weak var target: (any WKScriptMessageHandler)?

    init(_ target: any WKScriptMessageHandler) { self.target = target }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        target?.userContentController(controller, didReceive: message)
    }
}
