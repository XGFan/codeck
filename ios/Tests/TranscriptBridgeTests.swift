import Foundation
import UIKit
import WebKit
import XCTest
@testable import CoDeck

@MainActor
final class TranscriptBridgeTests: XCTestCase {

    // MARK: Scheme Path Filtering & URL Construction

    func testResolveSchemeRequestFiltering() {
        func req(_ urlString: String, method: String = "GET") -> URLRequest {
            var r = URLRequest(url: URL(string: urlString)!)
            r.httpMethod = method
            return r
        }

        XCTAssertEqual(
            HostWebBridge.resolveSchemeRequest(req("codeck-web://host/web/x.js")),
            HostWebBridge.SchemeTarget(path: "/web/x.js", query: nil)
        )
        XCTAssertEqual(
            HostWebBridge.resolveSchemeRequest(req("codeck-web://host/web/transcript.html?sessionId=s1&foo=a%20b")),
            HostWebBridge.SchemeTarget(path: "/web/transcript.html", query: "sessionId=s1&foo=a%20b")
        )
        XCTAssertEqual(
            HostWebBridge.transcriptURL(sessionId: "abc-1").absoluteString,
            "codeck-web://host/web/transcript.html?sessionId=abc-1"
        )

        let rejected: [(String, URLRequest)] = [
            ("non-/web path", req("codeck-web://host/v1/rpc")),
            ("traversal raw", req("codeck-web://host/web/../v1/rpc")),
            ("traversal encoded lower", req("codeck-web://host/web/%2e%2e/x")),
            ("traversal encoded upper", req("codeck-web://host/web/%2E%2E/x")),
            ("wrong authority", req("codeck-web://other/web/x.js")),
            ("wrong scheme", req("https://host/web/x.js")),
            ("POST method", req("codeck-web://host/web/x.js", method: "POST")),
        ]
        for (label, request) in rejected {
            XCTAssertNil(HostWebBridge.resolveSchemeRequest(request), label)
        }
    }

    func testSchemeHandlerRejectsBlockedRequestsWith404WithoutFetching() {
        let fetched = Counter()
        let handler = HostSchemeHandler(
            currentGeneration: { 1 },
            makeFetcher: {
                { _, _ in
                    fetched.increment()
                    return (Data(), HTTPURLResponse(url: URL(string: "https://10.0.0.1:38471/")!, statusCode: 200, httpVersion: nil, headerFields: nil)!)
                }
            }
        )

        let blockedURLs: [(String, String)] = [
            ("codeck-web://host/v1/rpc", "GET"),
            ("codeck-web://host/web/../v1/rpc", "GET"),
            ("codeck-web://host/web/%2e%2e/x", "GET"),
            ("codeck-web://evil/web/x.js", "GET"),
            ("codeck-web://host/web/x.js", "POST"),
        ]
        for (urlString, method) in blockedURLs {
            var request = URLRequest(url: URL(string: urlString)!)
            request.httpMethod = method
            let task = FakeURLSchemeTask(request: request)
            let web = WKWebView(frame: .zero)
            handler.webView(web, start: task)
            XCTAssertEqual((task.receivedResponse as? HTTPURLResponse)?.statusCode, 404, urlString)
            XCTAssertEqual(task.receivedResponse?.url, request.url, urlString)
            XCTAssertTrue(task.finished, urlString)
            XCTAssertNil(task.failedError, urlString)
        }
        XCTAssertEqual(fetched.value, 0)
    }

    func testSchemeHandlerForwardsHeadersAndUsesOriginalURL() async throws {
        let csp = "default-src 'self'; script-src 'self'"
        let handler = HostSchemeHandler(
            currentGeneration: { 3 },
            makeFetcher: {
                { path, query in
                    XCTAssertEqual(path, "/web/transcript.html")
                    XCTAssertEqual(query, "sessionId=s1")
                    let upstream = HTTPURLResponse(
                        url: URL(string: "https://192.168.1.20:38471/web/transcript.html?sessionId=s1")!,
                        statusCode: 200,
                        httpVersion: "HTTP/1.1",
                        headerFields: [
                            "Content-Type": "text/html; charset=utf-8",
                            "Content-Security-Policy": csp,
                            "Cache-Control": "no-store",
                            "X-Content-Type-Options": "nosniff",
                            "Set-Cookie": "should-not-forward=1",
                        ]
                    )!
                    return (Data("<html>ok</html>".utf8), upstream)
                }
            }
        )

        let customURL = URL(string: "codeck-web://host/web/transcript.html?sessionId=s1")!
        let task = FakeURLSchemeTask(request: URLRequest(url: customURL))
        handler.webView(WKWebView(frame: .zero), start: task)

        for _ in 0..<50 where !task.finished && task.failedError == nil {
            try await Task.sleep(for: .milliseconds(10))
        }

        let response = try XCTUnwrap(task.receivedResponse as? HTTPURLResponse)
        XCTAssertEqual(response.url, customURL)
        XCTAssertEqual(response.statusCode, 200)
        XCTAssertEqual(response.value(forHTTPHeaderField: "Content-Type"), "text/html; charset=utf-8")
        XCTAssertEqual(response.value(forHTTPHeaderField: "Content-Security-Policy"), csp)
        XCTAssertEqual(response.value(forHTTPHeaderField: "Cache-Control"), "no-store")
        XCTAssertEqual(response.value(forHTTPHeaderField: "X-Content-Type-Options"), "nosniff")
        XCTAssertNil(response.value(forHTTPHeaderField: "Set-Cookie"))
        XCTAssertEqual(String(decoding: task.receivedData, as: UTF8.self), "<html>ok</html>")
        XCTAssertTrue(task.finished)
        XCTAssertNil(task.failedError)
    }

    func testSchemeHandlerStopCancelsAndSuppressesLateCallbacks() async throws {
        let gate = BridgeAsyncGate()
        let handler = HostSchemeHandler(
            currentGeneration: { 1 },
            makeFetcher: {
                { _, _ in
                    await gate.wait()
                    let resp = HTTPURLResponse(
                        url: URL(string: "https://10.0.0.1:38471/web/app.js")!,
                        statusCode: 200,
                        httpVersion: nil,
                        headerFields: ["Content-Type": "text/javascript"]
                    )!
                    return (Data("console.log(1)".utf8), resp)
                }
            }
        )

        let customURL = URL(string: "codeck-web://host/web/app.js")!
        let task = FakeURLSchemeTask(request: URLRequest(url: customURL))
        let web = WKWebView(frame: .zero)
        handler.webView(web, start: task)
        handler.webView(web, stop: task)
        await gate.open()
        try await Task.sleep(for: .milliseconds(30))

        // Stopped task must not receive any response, finish, or failure callback.
        XCTAssertNil(task.receivedResponse)
        XCTAssertFalse(task.finished)
        XCTAssertNil(task.failedError)
    }

    func testSchemeHandlerDropsStaleGenerationResponse() async throws {
        let gen = Counter()
        gen.increment()
        let gate = BridgeAsyncGate()
        let handler = HostSchemeHandler(
            currentGeneration: { gen.value },
            makeFetcher: {
                { _, _ in
                    await gate.wait()
                    let resp = HTTPURLResponse(
                        url: URL(string: "https://10.0.0.1:38471/web/app.js")!,
                        statusCode: 200,
                        httpVersion: nil,
                        headerFields: ["Content-Type": "text/javascript"]
                    )!
                    return (Data("old".utf8), resp)
                }
            }
        )

        let task = FakeURLSchemeTask(request: URLRequest(url: URL(string: "codeck-web://host/web/app.js")!))
        handler.webView(WKWebView(frame: .zero), start: task)
        gen.increment()
        await gate.open()
        for _ in 0..<50 where task.failedError == nil && !task.finished {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertNil(task.receivedResponse)
        XCTAssertFalse(task.finished)
        XCTAssertEqual((task.failedError as? URLError)?.code, .cancelled)
    }

    // MARK: Origin & Navigation Locking

    func testOriginAndNavigationPolicy() {
        XCTAssertTrue(HostWebBridge.isAllowedOrigin(isMainFrame: true, protocol: "codeck-web", host: "host"))
        XCTAssertFalse(HostWebBridge.isAllowedOrigin(isMainFrame: false, protocol: "codeck-web", host: "host"))
        XCTAssertFalse(HostWebBridge.isAllowedOrigin(isMainFrame: true, protocol: "https", host: "host"))
        XCTAssertFalse(HostWebBridge.isAllowedOrigin(isMainFrame: true, protocol: "codeck-web", host: "other"))

        XCTAssertEqual(
            HostWebBridge.navigationDecision(
                url: URL(string: "codeck-web://host/web/transcript.html?sessionId=s1"),
                isMainFrame: true,
                isLinkActivated: false
            ),
            .allow
        )
        XCTAssertEqual(
            HostWebBridge.navigationDecision(
                url: URL(string: "codeck-web://host/web/other.html"),
                isMainFrame: true,
                isLinkActivated: false
            ),
            .cancel
        )
        let ext = URL(string: "https://example.com/docs")!
        XCTAssertEqual(
            HostWebBridge.navigationDecision(url: ext, isMainFrame: true, isLinkActivated: true),
            .openExternally(ext)
        )
        XCTAssertEqual(
            HostWebBridge.navigationDecision(url: ext, isMainFrame: true, isLinkActivated: false),
            .cancel
        )
        XCTAssertEqual(
            HostWebBridge.navigationDecision(
                url: URL(string: "codeck-web://host/web/sub.html"),
                isMainFrame: false,
                isLinkActivated: false
            ),
            .allow
        )
        XCTAssertEqual(
            HostWebBridge.navigationDecision(url: ext, isMainFrame: false, isLinkActivated: false),
            .cancel
        )
    }

    // MARK: End-to-End RPC Bridge (Success, Network Error, Subframe Forbidden)

    func testRPCBridgeEndToEndSuccessErrorAndForbiddenOrigin() async throws {
        let html = """
        <!doctype html>
        <html><body>
        <iframe src="codeck-web://host/web/sub.html"></iframe>
        <script>
          function report(item, payload) {
            window.webkit.messageHandlers.testReport.postMessage({ item, payload: JSON.stringify(payload) });
          }
          window.addEventListener("message", (e) => {
            if (e.data && e.data.fromSub) report("sub", e.data.reply);
          });
          (async () => {
            const ok = await window.webkit.messageHandlers.codeckRpc.postMessage({ method: "get_session", args: { sessionId: "s1" } });
            report("ok", ok);
            const fail = await window.webkit.messageHandlers.codeckRpc.postMessage({ method: "fail_net", args: {} });
            report("fail", fail);
          })();
        </script>
        </body></html>
        """
        let subHTML = """
        <!doctype html>
        <html><body><script>
          (async () => {
            const reply = await window.webkit.messageHandlers.codeckRpc.postMessage({ method: "get_session", args: {} });
            window.parent.postMessage({ fromSub: true, reply }, "*");
          })();
        </script></body></html>
        """

        let schemeHandler = HostSchemeHandler(
            currentGeneration: { 1 },
            makeFetcher: {
                { path, _ in
                    let body: String
                    if path == "/web/transcript.html" {
                        body = html
                    } else if path == "/web/sub.html" {
                        body = subHTML
                    } else {
                        let resp = HTTPURLResponse(url: URL(string: "https://10.0.0.1:38471\(path)")!, statusCode: 404, httpVersion: nil, headerFields: nil)!
                        return (Data(), resp)
                    }
                    let resp = HTTPURLResponse(
                        url: URL(string: "https://10.0.0.1:38471\(path)")!,
                        statusCode: 200,
                        httpVersion: "HTTP/1.1",
                        headerFields: ["Content-Type": "text/html; charset=utf-8"]
                    )!
                    return (Data(body.utf8), resp)
                }
            }
        )
        let rpcHandler = HostRPCMessageHandler(
            currentGeneration: { 1 },
            makeForwarder: {
                { method, args in
                    if method == "fail_net" {
                        throw HostError.unreachable("10.0.0.1:38471")
                    }
                    XCTAssertEqual(method, "get_session")
                    XCTAssertEqual(args, .object(["sessionId": .string("s1")]))
                    return Data(#"{"result":{"id":"s1","ok":true}}"#.utf8)
                }
            }
        )

        let expOK = expectation(description: "RPC ok reply")
        let expFail = expectation(description: "RPC network error reply")
        let expSub = expectation(description: "Subframe RPC forbidden reply")
        let collector = ReportCollector(expectations: ["ok": expOK, "fail": expFail, "sub": expSub])

        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(schemeHandler, forURLScheme: HostWebBridge.scheme)
        config.userContentController.addScriptMessageHandler(rpcHandler, contentWorld: .page, name: HostWebBridge.rpcHandlerName)
        config.userContentController.add(collector, name: "testReport")

        let frame = CGRect(x: 0, y: 0, width: 320, height: 480)
        let webView = WKWebView(frame: frame, configuration: config)
        var hostWindow: UIWindow?
        if let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first {
            let window = UIWindow(windowScene: scene)
            window.frame = frame
            let vc = UIViewController()
            vc.view.addSubview(webView)
            window.rootViewController = vc
            window.isHidden = false
            hostWindow = window
        }
        defer {
            schemeHandler.cancelAll()
            webView.stopLoading()
            webView.configuration.userContentController.removeAllScriptMessageHandlers()
            webView.removeFromSuperview()
            hostWindow?.isHidden = true
        }

        webView.load(URLRequest(url: HostWebBridge.transcriptURL(sessionId: "s1")))
        await fulfillment(of: [expOK, expFail, expSub], timeout: 10)

        let okJSON = try XCTUnwrap(parseJSONDict(collector.payloads["ok"]))
        let okResult = try XCTUnwrap(okJSON["result"] as? [String: Any])
        XCTAssertEqual(okResult["id"] as? String, "s1")
        XCTAssertEqual(okResult["ok"] as? Bool, true)

        let failJSON = try XCTUnwrap(parseJSONDict(collector.payloads["fail"]))
        let failErr = try XCTUnwrap(failJSON["error"] as? [String: Any])
        XCTAssertEqual(failErr["code"] as? String, "http")
        XCTAssertEqual(failErr["message"] as? String, "连不上 Host")

        let subJSON = try XCTUnwrap(parseJSONDict(collector.payloads["sub"]))
        let subErr = try XCTUnwrap(subJSON["error"] as? [String: Any])
        XCTAssertEqual(subErr["code"] as? String, "forbidden")
        XCTAssertEqual(subErr["message"] as? String, "不接受这个来源")
    }

    // MARK: Event Bridge (Ready Gate, Synthetic Hello, Generation & Multi-Subscriber)

    func testEventBridgePushesOnlyAfterReadyStartsWithHelloAndStopsOnGenerationChange() throws {
        let suite = "TranscriptBridgeTests.events.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = HostStore(defaults: defaults, keychain: BridgeMemoryKeychain(), allowLoopback: false)
        let model = HostModel(store: store)

        var hostGen = 1
        var pushed: [String] = []
        let bridge = TranscriptEventBridge(
            currentHostGeneration: { hostGen },
            subscribe: { model.subscribeRawEvents($0) },
            push: { pushed.append($0) }
        )

        bridge.beginLoad(hostGeneration: hostGen)

        // Before genuine `ready`: events must NOT be pushed.
        model.emitRawEvent(#"{"event":"live-changed-event","payload":{"live":[]}}"#)
        XCTAssertEqual(pushed, [])

        // After `ready`: synthetic hello is pushed first, followed by live Host events.
        bridge.pageDidReady()
        XCTAssertEqual(pushed, [HostWebBridge.syntheticHello])

        let liveMsg = #"{"event":"sessions-changed-event","payload":{"changed":["s1"],"removed":[],"projectsChanged":false}}"#
        model.emitRawEvent(liveMsg)
        XCTAssertEqual(pushed, [HostWebBridge.syntheticHello, liveMsg])

        // Second subscriber receives events independently; unsubscribing does not affect others.
        var otherReceived: [String] = []
        let unsubOther = model.subscribeRawEvents { otherReceived.append($0) }
        let msg2 = #"{"event":"prices-changed-event","payload":null}"#
        model.emitRawEvent(msg2)
        XCTAssertEqual(pushed, [HostWebBridge.syntheticHello, liveMsg, msg2])
        XCTAssertEqual(otherReceived, [msg2])
        unsubOther()

        // Host generation change stops pushing to the old page.
        hostGen = 2
        model.emitRawEvent(#"{"event":"prices-changed-event","payload":null}"#)
        XCTAssertEqual(pushed, [HostWebBridge.syntheticHello, liveMsg, msg2])

        // Reload (`beginLoad`) resets ready gate so events do not push until the new page says `ready`.
        bridge.beginLoad(hostGeneration: hostGen)
        model.emitRawEvent(#"{"event":"prices-changed-event","payload":null}"#)
        XCTAssertEqual(pushed.count, 3)

        bridge.pageDidReady()
        XCTAssertEqual(pushed.count, 4)
        XCTAssertEqual(pushed.last, HostWebBridge.syntheticHello)

        bridge.stop()
        model.emitRawEvent(#"{"event":"prices-changed-event","payload":null}"#)
        XCTAssertEqual(pushed.count, 4)
    }

    private func parseJSONDict(_ text: String?) -> [String: Any]? {
        guard let text, let data = text.data(using: .utf8) else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }
}

// MARK: - Test Doubles

private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var n = 0
    var value: Int { lock.withLock { n } }
    func increment() { lock.withLock { n += 1 } }
}

private actor BridgeAsyncGate {
    private var opened = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        if opened { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    func open() {
        opened = true
        for w in waiters { w.resume() }
        waiters.removeAll()
    }
}

private final class FakeURLSchemeTask: NSObject, WKURLSchemeTask {
    let request: URLRequest
    var receivedResponse: URLResponse?
    var receivedData = Data()
    var finished = false
    var failedError: Error?

    init(request: URLRequest) {
        self.request = request
    }

    func didReceive(_ response: URLResponse) {
        receivedResponse = response
    }

    func didReceive(_ data: Data) {
        receivedData.append(data)
    }

    func didFinish() {
        finished = true
    }

    func didFailWithError(_ error: any Error) {
        failedError = error
    }
}

@MainActor
private final class ReportCollector: NSObject, WKScriptMessageHandler {
    private let expectations: [String: XCTestExpectation]
    var payloads: [String: String] = [:]

    init(expectations: [String: XCTestExpectation]) {
        self.expectations = expectations
    }

    func userContentController(_: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any],
              let item = body["item"] as? String,
              let payload = body["payload"] as? String,
              payloads[item] == nil
        else { return }
        payloads[item] = payload
        expectations[item]?.fulfill()
    }
}

private final class BridgeMemoryKeychain: KeychainStoring, @unchecked Sendable {
    func read(service: String, account: String) -> String? { nil }
    func write(_ value: String, service: String, account: String) throws {}
    func delete(service: String, account: String) {}
}
