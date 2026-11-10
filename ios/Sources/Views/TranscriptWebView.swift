import SwiftUI
import WebKit

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
    }

    static func dismantleUIView(_ web: WKWebView, coordinator: Coordinator) {
        web.configuration.userContentController.removeScriptMessageHandler(forName: handlerName)
        coordinator.readyFallback?.cancel()
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        var parent: TranscriptWebView
        var loadedURL: URL?
        var loadedToken = 0
        var readyFallback: Task<Void, Never>?
        private var ready = false
        private var writtenInsets: [CGFloat]?

        init(parent: TranscriptWebView) { self.parent = parent }

        func load(_ web: WKWebView, url: URL, token: Int) {
            loadedURL = url
            loadedToken = token
            ready = false
            writtenInsets = nil
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
                if let web = message.webView { writeInsets(web, force: true) }
                markReady()
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
