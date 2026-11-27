import UIKit
import WebKit
import XCTest

@MainActor
final class SchemeBridgeTests: XCTestCase {
    private static let scheme = "codeck-web"
    private static let csp = "default-src 'self'; script-src 'self' 'unsafe-inline'; worker-src 'self' blob:; connect-src 'self'"

    private static let indexHTML = """
    <!doctype html>
    <html>
    <head>
      <meta charset="utf-8">
      <meta http-equiv="Content-Security-Policy" content="\(csp)">
      <title>SchemeBridgeTest</title>
    </head>
    <body>
    <script>
      function report(item, ok, detail) {
        window.webkit.messageHandlers.report.postMessage({ item, ok, detail });
      }

      document.addEventListener("securitypolicyviolation", (e) => {
        report("cspViolation", false, `${e.violatedDirective}: ${e.blockedURI}`);
      });

      window.codeckPage = {
        event(text) {
          try {
            const parsed = JSON.parse(text);
            report("nativeEvent", true, String(parsed.event));
          } catch (err) {
            report("nativeEvent", false, `throw: ${err}`);
          }
        }
      };

      report("ready", true, window.location.href);

      // (a) module Worker from custom scheme
      try {
        const w = new Worker("/web/worker.js", { type: "module" });
        w.onmessage = (e) => report("moduleWorker", true, String(e.data));
        w.onerror = (e) => report("moduleWorker", false, `onerror: ${e.message || e.type} (${e.filename}:${e.lineno})`);
      } catch (err) {
        report("moduleWorker", false, `throw: ${err}`);
      }

      // (b) blob Worker created from fetched script
      (async () => {
        try {
          const resp = await fetch("/web/blob-src.js");
          if (!resp.ok) {
            report("blobWorker", false, `fetch HTTP ${resp.status}`);
            return;
          }
          const text = await resp.text();
          const url = URL.createObjectURL(new Blob([text], { type: "text/javascript" }));
          const w = new Worker(url);
          w.onmessage = (e) => report("blobWorker", true, String(e.data));
          w.onerror = (e) => report("blobWorker", false, `onerror: ${e.message || e.type} (${e.filename}:${e.lineno})`);
        } catch (err) {
          report("blobWorker", false, `throw: ${err}`);
        }
      })();

      // (c) RPC round-trip via WKScriptMessageHandlerWithReply
      (async () => {
        try {
          const res = await window.webkit.messageHandlers.codeckRpc.postMessage({ method: "ping", args: { n: 1 } });
          report("rpc", true, JSON.stringify(res));
        } catch (err) {
          report("rpc", false, `throw: ${err}`);
        }
      })();
    </script>
    </body>
    </html>
    """

    private static let workerJS = """
    self.postMessage("module-ok");
    """

    private static let blobSrcJS = """
    self.postMessage("blob-ok");
    """

    func testCustomSchemeWorkersAndBidirectionalBridge() async {
        let harness = BridgeHarness(
            scheme: Self.scheme,
            resources: [
                "/web/index.html": Resource(
                    contentType: "text/html; charset=utf-8",
                    body: Data(Self.indexHTML.utf8),
                    extraHeaders: ["Content-Security-Policy": Self.csp]
                ),
                "/web/worker.js": Resource(
                    contentType: "text/javascript; charset=utf-8",
                    body: Data(Self.workerJS.utf8)
                ),
                "/web/blob-src.js": Resource(
                    contentType: "text/javascript; charset=utf-8",
                    body: Data(Self.blobSrcJS.utf8)
                ),
            ]
        )
        defer { harness.tearDown() }

        let expModule = expectation(description: "moduleWorker reported")
        let expBlob = expectation(description: "blobWorker reported")
        let expRPC = expectation(description: "rpc reported")
        let expNativeEvent = expectation(description: "nativeEvent reported")

        let state = TestState(expectations: [
            "moduleWorker": expModule,
            "blobWorker": expBlob,
            "rpc": expRPC,
            "nativeEvent": expNativeEvent,
        ])

        harness.onNavigationError = { error in
            state.navigationError = error
        }

        harness.onReport = { [weak harness] report in
            switch report.item {
            case "ready":
                guard let webView = harness?.webView else { return }
                webView.callAsyncJavaScript(
                    "window.codeckPage.event(e)",
                    arguments: ["e": "{\"event\":\"hello\"}"],
                    in: nil,
                    in: .page
                ) { result in
                    if case .failure(let error) = result {
                        state.record(Report(item: "nativeEvent", ok: false, detail: "callAsyncJavaScript failed: \(error)"))
                    }
                }
            case "cspViolation":
                state.cspViolations.append(report.detail)
            default:
                state.record(report)
            }
        }

        harness.load(URL(string: "\(Self.scheme)://app/web/index.html")!)

        await fulfillment(of: [expModule, expBlob, expRPC, expNativeEvent], timeout: 10)

        XCTAssertNil(state.navigationError, "Unexpected navigation error: \(String(describing: state.navigationError))")
        XCTAssertEqual(state.cspViolations, [], "Unexpected CSP violations: \(state.cspViolations)")
        XCTAssertEqual(harness.servedPaths, ["/web/index.html", "/web/worker.js", "/web/blob-src.js"])

        XCTAssertEqual(state.results["moduleWorker"], Report(item: "moduleWorker", ok: true, detail: "module-ok"))
        XCTAssertEqual(state.results["blobWorker"], Report(item: "blobWorker", ok: true, detail: "blob-ok"))
        XCTAssertEqual(state.results["rpc"], Report(item: "rpc", ok: true, detail: "{\"result\":{\"pong\":1}}"))
        XCTAssertEqual(state.results["nativeEvent"], Report(item: "nativeEvent", ok: true, detail: "hello"))
    }
}

private struct Resource {
    let contentType: String
    let body: Data
    var extraHeaders: [String: String] = [:]
}

private struct Report: Equatable {
    let item: String
    let ok: Bool
    let detail: String
}

@MainActor
private final class TestState {
    private let expectations: [String: XCTestExpectation]
    var results: [String: Report] = [:]
    var cspViolations: [String] = []
    var navigationError: Error?

    init(expectations: [String: XCTestExpectation]) {
        self.expectations = expectations
    }

    func record(_ report: Report) {
        guard results[report.item] == nil else { return }
        results[report.item] = report
        expectations[report.item]?.fulfill()
    }
}

@MainActor
private final class MemorySchemeHandler: NSObject, WKURLSchemeHandler {
    private let resources: [String: Resource]
    private(set) var servedPaths: Set<String> = []

    init(resources: [String: Resource]) {
        self.resources = resources
    }

    func webView(_: WKWebView, start urlSchemeTask: any WKURLSchemeTask) {
        guard let url = urlSchemeTask.request.url else {
            urlSchemeTask.didFailWithError(URLError(.badURL))
            return
        }

        if let resource = resources[url.path] {
            servedPaths.insert(url.path)
            var headers = resource.extraHeaders
            headers["Content-Type"] = resource.contentType
            headers["Content-Length"] = "\(resource.body.count)"
            let response = HTTPURLResponse(
                url: url,
                statusCode: 200,
                httpVersion: "HTTP/1.1",
                headerFields: headers
            )!
            urlSchemeTask.didReceive(response)
            urlSchemeTask.didReceive(resource.body)
            urlSchemeTask.didFinish()
        } else {
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
            urlSchemeTask.didReceive(response)
            urlSchemeTask.didReceive(body)
            urlSchemeTask.didFinish()
        }
    }

    func webView(_: WKWebView, stop _: any WKURLSchemeTask) {}
}

@MainActor
private final class ReportMessageHandler: NSObject, WKScriptMessageHandler {
    var onReport: ((Report) -> Void)?

    func userContentController(_: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any],
              let item = body["item"] as? String,
              let ok = body["ok"] as? Bool,
              let detail = body["detail"] as? String
        else { return }
        onReport?(Report(item: item, ok: ok, detail: detail))
    }
}

@MainActor
private final class RPCMessageHandler: NSObject, WKScriptMessageHandlerWithReply {
    func userContentController(
        _: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void
    ) {
        guard let body = message.body as? [String: Any],
              let method = body["method"] as? String,
              method == "ping",
              let args = body["args"] as? [String: Any],
              let n = (args["n"] as? NSNumber)?.intValue
        else {
            replyHandler(nil, "unexpected rpc payload: \(message.body)")
            return
        }
        replyHandler(["result": ["pong": n]], nil)
    }
}

@MainActor
private final class BridgeHarness: NSObject, WKNavigationDelegate {
    let webView: WKWebView
    private let schemeHandler: MemorySchemeHandler
    private let reportHandler = ReportMessageHandler()
    private let rpcHandler = RPCMessageHandler()
    private var hostWindow: UIWindow?

    var servedPaths: Set<String> { schemeHandler.servedPaths }
    var onReport: ((Report) -> Void)? {
        get { reportHandler.onReport }
        set { reportHandler.onReport = newValue }
    }
    var onNavigationError: ((Error) -> Void)?

    init(scheme: String, resources: [String: Resource]) {
        self.schemeHandler = MemorySchemeHandler(resources: resources)
        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(schemeHandler, forURLScheme: scheme)
        config.userContentController.add(reportHandler, name: "report")
        config.userContentController.addScriptMessageHandler(rpcHandler, contentWorld: .page, name: "codeckRpc")

        let frame = CGRect(x: 0, y: 0, width: 320, height: 480)
        self.webView = WKWebView(frame: frame, configuration: config)
        super.init()

        webView.navigationDelegate = self
        if let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first {
            let window = UIWindow(windowScene: scene)
            window.frame = frame
            let vc = UIViewController()
            vc.view.addSubview(webView)
            window.rootViewController = vc
            window.isHidden = false
            self.hostWindow = window
        }
    }

    func load(_ url: URL) {
        webView.load(URLRequest(url: url))
    }

    func tearDown() {
        webView.stopLoading()
        webView.navigationDelegate = nil
        webView.configuration.userContentController.removeAllScriptMessageHandlers()
        webView.removeFromSuperview()
        hostWindow?.isHidden = true
        hostWindow = nil
    }

    func webView(_: WKWebView, didFail _: WKNavigation!, withError error: Error) {
        onNavigationError?(error)
    }

    func webView(_: WKWebView, didFailProvisionalNavigation _: WKNavigation!, withError error: Error) {
        onNavigationError?(error)
    }
}
