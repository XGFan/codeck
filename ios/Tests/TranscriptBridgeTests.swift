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
            HostWebBridge.resolveSchemeRequest(req("codeck-web://host/web/a%20b/transcript.html?sessionId=s1&foo=a%20b")),
            HostWebBridge.SchemeTarget(path: "/web/a%20b/transcript.html", query: "sessionId=s1&foo=a%20b")
        )
        XCTAssertEqual(
            HostWebBridge.resolveSchemeRequest(req("codeck-web://host/web/transcript.html?")),
            HostWebBridge.SchemeTarget(path: "/web/transcript.html", query: "")
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
            ("traversal encoded slash", req("codeck-web://host/web/..%2Fv1/rpc")),
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
            boundGeneration: 1,
            currentGeneration: { 1 },
            fetcher: { _, _ in
                fetched.increment()
                return (Data(), HTTPURLResponse(url: URL(string: "https://10.0.0.1:38471/")!, statusCode: 200, httpVersion: nil, headerFields: nil)!)
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
            let http = task.receivedResponse as? HTTPURLResponse
            XCTAssertEqual(http?.statusCode, 404, urlString)
            XCTAssertEqual(http?.value(forHTTPHeaderField: "Cache-Control"), "no-store", urlString)
            XCTAssertEqual(task.receivedResponse?.url, request.url, urlString)
            XCTAssertTrue(task.finished, urlString)
            XCTAssertNil(task.failedError, urlString)
        }
        XCTAssertEqual(fetched.value, 0)
    }

    func testSchemeHandlerForwardsHeadersForcesNoStoreAndUsesOriginalURL() async throws {
        let csp = "default-src 'self'; script-src 'self'"
        let handler = HostSchemeHandler(
            boundGeneration: 3,
            currentGeneration: { 3 },
            fetcher: { path, query in
                XCTAssertEqual(path, "/web/a%20b/transcript.html")
                XCTAssertEqual(query, "sessionId=s%201")
                let upstream = HTTPURLResponse(
                    url: URL(string: "https://192.168.1.20:38471/web/a%20b/transcript.html?sessionId=s%201")!,
                    statusCode: 200,
                    httpVersion: "HTTP/1.1",
                    headerFields: [
                        "Content-Type": "text/html; charset=utf-8",
                        "Content-Security-Policy": csp,
                        // Host sent a long-lived cache header; bridge must override with no-store.
                        "Cache-Control": "public, max-age=31536000, immutable",
                        "X-Content-Type-Options": "nosniff",
                        "Set-Cookie": "should-not-forward=1",
                    ]
                )!
                return (Data("<html>ok</html>".utf8), upstream)
            }
        )

        let customURL = URL(string: "codeck-web://host/web/a%20b/transcript.html?sessionId=s%201")!
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
            boundGeneration: 1,
            currentGeneration: { 1 },
            fetcher: { _, _ in
                await gate.wait()
                let resp = HTTPURLResponse(
                    url: URL(string: "https://10.0.0.1:38471/web/app.js")!,
                    statusCode: 200,
                    httpVersion: nil,
                    headerFields: ["Content-Type": "text/javascript"]
                )!
                return (Data("console.log(1)".utf8), resp)
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

    func testSchemeHandlerDropsStaleGenerationResponseWith404() async throws {
        let gen = Counter()
        gen.increment()
        let gate = BridgeAsyncGate()
        let fetchCount = Counter()
        let handler = HostSchemeHandler(
            boundGeneration: 1,
            currentGeneration: { gen.value },
            fetcher: { _, _ in
                fetchCount.increment()
                await gate.wait()
                let resp = HTTPURLResponse(
                    url: URL(string: "https://10.0.0.1:38471/web/app.js")!,
                    statusCode: 200,
                    httpVersion: nil,
                    headerFields: ["Content-Type": "text/javascript"]
                )!
                return (Data("old".utf8), resp)
            }
        )

        // In-flight request completes after generation changed → 404 (no-store).
        let task = FakeURLSchemeTask(request: URLRequest(url: URL(string: "codeck-web://host/web/app.js")!))
        handler.webView(WKWebView(frame: .zero), start: task)
        gen.increment()
        await gate.open()
        for _ in 0..<50 where task.failedError == nil && !task.finished {
            try await Task.sleep(for: .milliseconds(10))
        }
        let response = try XCTUnwrap(task.receivedResponse as? HTTPURLResponse)
        XCTAssertEqual(response.statusCode, 404)
        XCTAssertEqual(response.value(forHTTPHeaderField: "Cache-Control"), "no-store")
        XCTAssertTrue(task.finished)
        XCTAssertNil(task.failedError)
        XCTAssertEqual(fetchCount.value, 1)

        // New request arriving after generation changed → 404 immediately without calling fetcher.
        let lateTask = FakeURLSchemeTask(request: URLRequest(url: URL(string: "codeck-web://host/web/app2.js")!))
        handler.webView(WKWebView(frame: .zero), start: lateTask)
        XCTAssertEqual((lateTask.receivedResponse as? HTTPURLResponse)?.statusCode, 404)
        XCTAssertTrue(lateTask.finished)
        XCTAssertEqual(fetchCount.value, 1)
    }

    // MARK: Origin & Navigation Locking

    func testOriginAndNavigationPolicy() {
        XCTAssertTrue(HostWebBridge.isAllowedOrigin(isMainFrame: true, protocol: "codeck-web", host: "host"))
        XCTAssertFalse(HostWebBridge.isAllowedOrigin(isMainFrame: true, protocol: "codeck-web", host: "host", port: 8080))
        XCTAssertFalse(HostWebBridge.isAllowedOrigin(isMainFrame: false, protocol: "codeck-web", host: "host"))
        XCTAssertFalse(HostWebBridge.isAllowedOrigin(isMainFrame: true, protocol: "https", host: "host"))
        XCTAssertFalse(HostWebBridge.isAllowedOrigin(isMainFrame: true, protocol: "codeck-web", host: "other"))

        XCTAssertTrue(
            HostWebBridge.matchesPageURL(
                URL(string: "codeck-web://host/web/transcript.html?sessionId=s1#top"),
                expected: URL(string: "codeck-web://host/web/transcript.html?sessionId=s1")
            )
        )
        XCTAssertFalse(
            HostWebBridge.matchesPageURL(
                URL(string: "codeck-web://host/web/transcript.html?sessionId=s1"),
                expected: URL(string: "codeck-web://host/web/transcript.html?sessionId=s2")
            )
        )

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
            boundGeneration: 1,
            currentGeneration: { 1 },
            fetcher: { path, _ in
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
        )
        let rpcHandler = HostRPCMessageHandler(
            boundGeneration: 1,
            currentGeneration: { 1 },
            forwarder: { method, args in
                if method == "fail_net" {
                    throw HostError.unreachable("10.0.0.1:38471")
                }
                XCTAssertEqual(method, "get_session")
                XCTAssertEqual(args, .object(["sessionId": .string("s1")]))
                let resp = HTTPURLResponse(url: URL(string: "https://10.0.0.1:38471/v1/rpc")!, statusCode: 200, httpVersion: nil, headerFields: nil)!
                return (Data(#"{"result":{"id":"s1","ok":true}}"#.utf8), resp)
            }
        )

        let expOK = expectation(description: "RPC ok reply")
        let expFail = expectation(description: "RPC network error reply")
        let expSub = expectation(description: "Subframe RPC forbidden reply")
        let collector = ReportCollector(expectations: ["ok": expOK, "fail": expFail, "sub": expSub])

        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.setURLSchemeHandler(schemeHandler, forURLScheme: HostWebBridge.scheme)
        config.userContentController.addScriptMessageHandler(rpcHandler, contentWorld: .page, name: HostWebBridge.rpcHandlerName)
        config.userContentController.add(collector, name: "testReport")

        let frame = CGRect(x: 0, y: 0, width: 320, height: 480)
        let webView = WKWebView(frame: frame, configuration: config)
        rpcHandler.expectedWebView = webView
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

    // MARK: Regression — Stale WebView Never Reaches New Host Client After Generation Switch

    func testStaleWebViewRPCAndResourcesNeverReachNewHostClient() async throws {
        let suite = "TranscriptBridgeTests.hostSwitch.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = BridgeMemoryKeychain()
        let store = HostStore(defaults: defaults, keychain: keychain, allowLoopback: false)

        let fpA = "vu8xtlO5XgT96RXqPEF3L6izmPVudCDb-uomlgZtiBQ"
        let fpB = "6o220Q-JEKn5p9inkMLss_p7OlCPpZlM1NMxXHLQLh8"
        let hostA = PairedHost(
            address: HostAddress(host: "10.0.0.1", port: 38471),
            fingerprint: fpA,
            deviceId: "devA",
            token: "tokA"
        )
        try store.save(hostA)

        let html = """
        <!doctype html>
        <html><body><script>
          window.runRPC = async function(tag) {
            const reply = await window.webkit.messageHandlers.codeckRpc.postMessage({ method: "get_session", args: { sessionId: tag } });
            window.webkit.messageHandlers.testReport.postMessage({ item: tag, payload: JSON.stringify(reply) });
          };
          window.webkit.messageHandlers.testReport.postMessage({ item: "loaded", payload: "{}" });
        </script></body></html>
        """

        let hostARPCs = Counter()
        let hostBRPCs = Counter()
        let hostBAssets = Counter()
        let stubSession = BridgeStubURLProtocol.makeSession()
        BridgeStubURLProtocol.handler = { req in
            let host = req.url?.host ?? ""
            let path = req.url?.path ?? ""
            if host == "10.0.0.1" {
                if path == "/web/transcript.html" {
                    return (200, Data(html.utf8), ["Content-Type": "text/html; charset=utf-8"])
                }
                if path == "/v1/rpc" {
                    hostARPCs.increment()
                    return (200, Data(#"{"result":{"from":"A"}}"#.utf8), ["Content-Type": "application/json"])
                }
                return (200, Data(#"{"product":"CoDeck","apiVersion":2}"#.utf8), [:])
            } else if host == "10.0.0.2" {
                if path.hasPrefix("/web/") {
                    hostBAssets.increment()
                    return (200, Data("B-asset".utf8), ["Content-Type": "text/plain"])
                }
                if path == "/v1/rpc" {
                    hostBRPCs.increment()
                    return (200, Data(#"{"result":{"from":"B"}}"#.utf8), ["Content-Type": "application/json"])
                }
                return (200, Data(#"{"product":"CoDeck","apiVersion":2}"#.utf8), [:])
            }
            return (404, Data(), [:])
        }

        let model = HostModel(
            store: store,
            clientFactory: { host, _ in HostClient(host: host, session: stubSession) },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )
        model.pause()

        // Create handlers bound to Host A (gen 1).
        let schemeHandlerA = HostSchemeHandler(model: model)
        let rpcHandlerA = HostRPCMessageHandler(model: model)

        let expLoaded = expectation(description: "Host A page loaded")
        let expAfterSwitch = expectation(description: "Stale RPC rejected after switch")
        let collector = ReportCollector(expectations: ["loaded": expLoaded, "after": expAfterSwitch])

        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.setURLSchemeHandler(schemeHandlerA, forURLScheme: HostWebBridge.scheme)
        config.userContentController.addScriptMessageHandler(rpcHandlerA, contentWorld: .page, name: HostWebBridge.rpcHandlerName)
        config.userContentController.add(collector, name: "testReport")

        let frame = CGRect(x: 0, y: 0, width: 320, height: 480)
        let webView = WKWebView(frame: frame, configuration: config)
        rpcHandlerA.expectedWebView = webView
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
            schemeHandlerA.cancelAll()
            webView.stopLoading()
            webView.configuration.userContentController.removeAllScriptMessageHandlers()
            webView.removeFromSuperview()
            hostWindow?.isHidden = true
            BridgeStubURLProtocol.handler = nil
        }

        webView.load(URLRequest(url: HostWebBridge.transcriptURL(sessionId: "s1")))
        await fulfillment(of: [expLoaded], timeout: 10)

        // Switch HostModel to Host B (gen 2, new client B).
        let linkB = PairingLink(hosts: ["10.0.0.2"], port: 38471, fingerprint: fpB, key: "AbCdEf0123456789-_xyZw")
        let depsB = PairingDependencies(
            checkHealth: { _, _, _ in HostHealth(product: "CoDeck", apiVersion: 2, version: nil) },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in ("devB", "tokB") },
            save: { try store.save($0) }
        )
        _ = try await model.pair(with: linkB, dependencies: depsB)
        model.pause()
        // Wait briefly for any initial refresh on Host B to settle, then reset counters.
        try await Task.sleep(for: .milliseconds(30))
        let hostARPCsBefore = hostARPCs.value
        let hostBRPCsBefore = hostBRPCs.value

        // Trigger RPC from the OLD Host A WebView after switching to Host B.
        webView.evaluateJavaScript("void window.runRPC('after')", completionHandler: nil)
        await fulfillment(of: [expAfterSwitch], timeout: 10)

        let afterJSON = try XCTUnwrap(parseJSONDict(collector.payloads["after"]))
        let afterErr = try XCTUnwrap(afterJSON["error"] as? [String: Any])
        XCTAssertEqual(afterErr["code"] as? String, "forbidden")
        XCTAssertEqual(afterErr["message"] as? String, "Host 已更换")
        // Neither client A nor client B received the stale RPC.
        XCTAssertEqual(hostARPCs.value, hostARPCsBefore)
        XCTAssertEqual(hostBRPCs.value, hostBRPCsBefore)

        // Scheme request on the OLD handler returns 404 without touching Host B.
        let lateAsset = FakeURLSchemeTask(request: URLRequest(url: URL(string: "codeck-web://host/web/secret.js")!))
        schemeHandlerA.webView(webView, start: lateAsset)
        XCTAssertEqual((lateAsset.receivedResponse as? HTTPURLResponse)?.statusCode, 404)
        XCTAssertEqual((lateAsset.receivedResponse as? HTTPURLResponse)?.value(forHTTPHeaderField: "Cache-Control"), "no-store")
        XCTAssertEqual(hostBAssets.value, 0)
    }

    // MARK: Regression — Bridge 401 Preserves Raw Status/Body & Triggers HostModel needsPairing

    func testBridge401PreservesStatusAndEnvelopeAndTriggersNeedsPairing() async throws {
        let suite = "TranscriptBridgeTests.bridge401.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = HostStore(defaults: defaults, keychain: BridgeMemoryKeychain(), allowLoopback: false)

        let fp = "vu8xtlO5XgT96RXqPEF3L6izmPVudCDb-uomlgZtiBQ"
        let host = PairedHost(
            address: HostAddress(host: "10.0.0.1", port: 38471),
            fingerprint: fp,
            deviceId: "dev1",
            token: "tok1"
        )
        try store.save(host)

        let unauthorizedEnvelope = #"{"error":{"code":"unauthorized","message":"这台设备没有配对或已被移除，请重新扫码配对。"}}"#
        let stubSession = BridgeStubURLProtocol.makeSession()
        // 1. Scheme handler 401: preserves 401 + body + no-store and triggers model.needsPairing.
        BridgeStubURLProtocol.handler = { req in
            if req.url?.path == "/web/asset.js" {
                return (401, Data(unauthorizedEnvelope.utf8), ["Content-Type": "application/json", "Cache-Control": "public, max-age=60"])
            }
            return (200, Data(#"{"product":"CoDeck","apiVersion":2}"#.utf8), [:])
        }
        let model1 = HostModel(
            store: store,
            clientFactory: { host, _ in HostClient(host: host, session: stubSession) },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )
        model1.pause()
        XCTAssertFalse(model1.needsPairing)

        let schemeHandler = HostSchemeHandler(model: model1)
        let assetTask = FakeURLSchemeTask(request: URLRequest(url: URL(string: "codeck-web://host/web/asset.js")!))
        schemeHandler.webView(WKWebView(frame: .zero), start: assetTask)
        for _ in 0..<50 where !assetTask.finished && assetTask.failedError == nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        let assetResp = try XCTUnwrap(assetTask.receivedResponse as? HTTPURLResponse)
        XCTAssertEqual(assetResp.statusCode, 401)
        XCTAssertEqual(assetResp.value(forHTTPHeaderField: "Cache-Control"), "no-store")
        XCTAssertEqual(String(decoding: assetTask.receivedData, as: UTF8.self), unauthorizedEnvelope)
        XCTAssertTrue(model1.needsPairing)
        XCTAssertEqual(model1.problem, HostError.removed.errorDescription)

        // 2. RPC bridge 401: returns Host's JSON envelope verbatim and triggers model.needsPairing.
        let pageHTML = """
        <!doctype html>
        <html><body><script>
          (async () => {
            const reply = await window.webkit.messageHandlers.codeckRpc.postMessage({ method: "get_session", args: { sessionId: "s1" } });
            window.webkit.messageHandlers.testReport.postMessage({ item: "rpc401", payload: JSON.stringify(reply) });
          })();
        </script></body></html>
        """
        BridgeStubURLProtocol.handler = { req in
            switch req.url?.path {
            case "/web/transcript.html":
                return (200, Data(pageHTML.utf8), ["Content-Type": "text/html; charset=utf-8"])
            case "/v1/rpc":
                return (401, Data(unauthorizedEnvelope.utf8), ["Content-Type": "application/json"])
            default:
                return (200, Data(#"{"product":"CoDeck","apiVersion":2}"#.utf8), [:])
            }
        }
        let model2 = HostModel(
            store: store,
            clientFactory: { host, _ in HostClient(host: host, session: stubSession) },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )
        model2.pause()

        let schemeHandler2 = HostSchemeHandler(model: model2)
        let rpcHandler2 = HostRPCMessageHandler(model: model2)
        let exp401 = expectation(description: "RPC 401 envelope forwarded")
        let collector = ReportCollector(expectations: ["rpc401": exp401])

        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.setURLSchemeHandler(schemeHandler2, forURLScheme: HostWebBridge.scheme)
        config.userContentController.addScriptMessageHandler(rpcHandler2, contentWorld: .page, name: HostWebBridge.rpcHandlerName)
        config.userContentController.add(collector, name: "testReport")

        let frame = CGRect(x: 0, y: 0, width: 320, height: 480)
        let webView = WKWebView(frame: frame, configuration: config)
        rpcHandler2.expectedWebView = webView
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
            schemeHandler2.cancelAll()
            webView.stopLoading()
            webView.configuration.userContentController.removeAllScriptMessageHandlers()
            webView.removeFromSuperview()
            hostWindow?.isHidden = true
            BridgeStubURLProtocol.handler = nil
        }

        webView.load(URLRequest(url: HostWebBridge.transcriptURL(sessionId: "s1")))
        await fulfillment(of: [exp401], timeout: 10)

        let rpcJSON = try XCTUnwrap(parseJSONDict(collector.payloads["rpc401"]))
        let rpcErr = try XCTUnwrap(rpcJSON["error"] as? [String: Any])
        XCTAssertEqual(rpcErr["code"] as? String, "unauthorized")
        XCTAssertEqual(rpcErr["message"] as? String, "这台设备没有配对或已被移除，请重新扫码配对。")
        XCTAssertTrue(model2.needsPairing)
        XCTAssertEqual(model2.problem, HostError.removed.errorDescription)
    }

    // MARK: Event Bridge (Commit/Ready Gate, URL Matching, Synthetic Hello, Provisional Nav & Generation)

    func testEventBridgePushesOnlyAfterCommitAndMatchingReadyAndStopsOnProvisionalNavAndGenerationChange() throws {
        let suite = "TranscriptBridgeTests.events.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = HostStore(defaults: defaults, keychain: BridgeMemoryKeychain(), allowLoopback: false)
        let model = HostModel(store: store)

        let url1 = HostWebBridge.transcriptURL(sessionId: "s1")
        let url2 = HostWebBridge.transcriptURL(sessionId: "s2")
        var hostGen = 1
        var pushed: [String] = []
        let bridge = TranscriptEventBridge(
            boundHostGeneration: 1,
            currentHostGeneration: { hostGen },
            subscribe: { model.subscribeRawEvents($0) },
            push: { pushed.append($0) }
        )

        bridge.didStartNavigation()

        // Before `didCommit`: `ready` must be ignored and no events pushed.
        XCTAssertFalse(bridge.pageDidReady(frameURL: url1))
        model.emitRawEvent(#"{"event":"live-changed-event","payload":{"live":[]}}"#)
        XCTAssertEqual(pushed, [])

        // After `didCommit` for url1: `ready` from a different URL (e.g. stale page url2) must be rejected.
        bridge.didCommit(url: url1)
        XCTAssertFalse(bridge.pageDidReady(frameURL: url2))
        XCTAssertEqual(pushed, [])

        // Matching `ready` after `didCommit`: synthetic hello is pushed first, followed by live Host events.
        XCTAssertTrue(bridge.pageDidReady(frameURL: url1))
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

        // Starting a provisional navigation stops pushing immediately and rejects `ready` until `didCommit`.
        bridge.didStartNavigation()
        model.emitRawEvent(#"{"event":"prices-changed-event","payload":null}"#)
        XCTAssertEqual(pushed.count, 3)
        XCTAssertFalse(bridge.pageDidReady(frameURL: url1))

        bridge.didCommit(url: url2)
        XCTAssertTrue(bridge.pageDidReady(frameURL: url2))
        XCTAssertEqual(pushed.count, 4)
        XCTAssertEqual(pushed.last, HostWebBridge.syntheticHello)

        // Host generation change permanently revokes this bridge (boundHostGeneration == 1).
        hostGen = 2
        model.emitRawEvent(#"{"event":"prices-changed-event","payload":null}"#)
        XCTAssertEqual(pushed.count, 4)
        bridge.didCommit(url: url2)
        XCTAssertFalse(bridge.pageDidReady(frameURL: url2))
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
    private let lock = NSLock()
    private var storage: [String: String] = [:]

    func read(service: String, account: String) -> String? {
        lock.withLock { storage["\(service):\(account)"] }
    }

    func write(_ value: String, service: String, account: String) throws {
        lock.withLock { storage["\(service):\(account)"] = value }
    }

    func delete(service: String, account: String) {
        lock.withLock { _ = storage.removeValue(forKey: "\(service):\(account)") }
    }
}

private final class BridgeStubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: ((URLRequest) throws -> (Int, Data, [String: String]))?

    static func makeSession() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [BridgeStubURLProtocol.self]
        return URLSession(configuration: config)
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let handler = Self.handler else {
            client?.urlProtocol(self, didFailWithError: URLError(.unknown))
            return
        }
        do {
            let (status, body, headers) = try handler(request)
            let response = HTTPURLResponse(
                url: request.url!,
                statusCode: status,
                httpVersion: "HTTP/1.1",
                headerFields: headers
            )!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: body)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}
