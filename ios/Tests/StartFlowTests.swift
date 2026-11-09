import XCTest
@testable import CoDeck

/// Mirrors src/features/connect/fork.test.ts and resume.test.ts, plus following a started connection.
final class StartFlowTests: XCTestCase {
    // MARK: forkTitle

    func testForkTitleAppendsFork() {
        XCTAssertEqual(StartFlow.forkTitle("排查 Redis 连接池泄漏", taken: []), "排查 Redis 连接池泄漏 (Fork)")
    }

    func testForkTitleTakesTheFirstFreeNumber() {
        XCTAssertEqual(StartFlow.forkTitle("拆分", taken: ["拆分 (Fork)"]), "拆分 (Fork 2)")
        XCTAssertEqual(StartFlow.forkTitle("拆分", taken: ["拆分 (Fork)", "拆分 (Fork 2)", "拆分 (Fork 4)"]), "拆分 (Fork 3)")
    }

    func testForkOfAForkUsesItsOriginsName() {
        XCTAssertEqual(StartFlow.forkTitle("拆分 (Fork)", taken: ["拆分 (Fork)"]), "拆分 (Fork 2)")
        XCTAssertEqual(StartFlow.forkTitle("拆分 (Fork 3)", taken: []), "拆分 (Fork)")
    }

    func testForkTitleIsOneLine() {
        XCTAssertEqual(StartFlow.forkTitle("第一行\n第二行  ", taken: []), "第一行 第二行 (Fork)")
    }

    // MARK: Offers

    func testResumeOffer() {
        let ok = StartOffer.resume(live: false, cwd: "/work/app", cwdMissing: false, ccrpcError: nil)
        XCTAssertEqual(ok, .enabled)
        XCTAssertEqual(StartOffer.resume(live: true, cwd: "/work/app", cwdMissing: false, ccrpcError: nil), .hidden)
        XCTAssertEqual(StartOffer.resume(live: false, cwd: nil, cwdMissing: false, ccrpcError: nil), .disabled("不知道这个 Session 的启动目录"))
        XCTAssertEqual(StartOffer.resume(live: false, cwd: "/work/app", cwdMissing: true, ccrpcError: nil), .disabled("目录 /work/app 已不存在"))
        XCTAssertEqual(StartOffer.resume(live: false, cwd: "/work/app", cwdMissing: false, ccrpcError: "运行不了 ccrpc"), .disabled("运行不了 ccrpc"))
    }

    func testForkOfferAlsoForARunningSession() {
        XCTAssertEqual(StartOffer.fork(cwd: "/work/app", cwdMissing: false, ccrpcError: nil), .enabled)
        XCTAssertEqual(StartOffer.fork(cwd: nil, cwdMissing: false, ccrpcError: nil), .disabled("不知道这个 Session 的启动目录"))
        XCTAssertEqual(StartOffer.fork(cwd: "/work/app", cwdMissing: true, ccrpcError: nil), .disabled("目录 /work/app 已不存在"))
    }

    // MARK: resumeFailure

    private let screen = [
        #"guofan@MBP work % env "$TOH_ARG_1" "$TOH_ARG_2" claude --resume 597821af-f8eb-49"#,
        "aa-8fd8-cf72e4e0b5c3 --plugin-dir /tmp/p/ccrpc",
        "No conversation found with session ID: 597821af-f8eb-49aa-8fd8-cf72e4e0b5c3",
        "guofan@MBP work %",
        "",
    ]

    func testResumeFailureTakesClaudesErrorLine() {
        let error = "No conversation found with session ID: 597821af-f8eb-49aa-8fd8-cf72e4e0b5c3"
        XCTAssertEqual(StartFlow.resumeFailure(screen, cursorRow: 3), error)
        XCTAssertEqual(StartFlow.resumeFailure(Array(screen.prefix(3)) + ["~/work on main", "❯", ""], cursorRow: 4), error)
        XCTAssertEqual(StartFlow.resumeFailure(Array(screen.prefix(3)), cursorRow: 3), error)
    }

    func testResumeFailureElseTheLastLineAboveTheCursor() {
        XCTAssertEqual(StartFlow.resumeFailure(["% claude --resume x", "Something unexpected", "% ", ""], cursorRow: 2), "Something unexpected")
        XCTAssertEqual(StartFlow.resumeFailure(["", " "], cursorRow: 0), "claude 退出了")
    }

    // MARK: Following a started connection

    private func conn(session: String?, live: Bool = true, stderr: String? = nil, notice: String? = nil) -> ConnState {
        var c = ConnState(conn: "c1")
        c.sessionId = session
        c.ready = session != nil
        if !live { c.procExit = .init(code: 1, stderr: stderr ?? "") }
        if let notice { c.notices = [.init(id: 1, text: notice)] }
        return c
    }

    func testFollowsUntilClaudeNamesTheSession() {
        XCTAssertEqual(StartFlow.outcome(.new(cwd: "/w"), nil), .waiting)
        XCTAssertEqual(StartFlow.outcome(.new(cwd: "/w"), conn(session: nil)), .waiting)
        XCTAssertEqual(StartFlow.outcome(.new(cwd: "/w"), conn(session: "s9")), .ready(sessionId: "s9"))
        XCTAssertEqual(StartFlow.outcome(.resume(sessionId: "s1"), conn(session: "s1")), .ready(sessionId: "s1"))
    }

    func testAForkIsReadyOnlyOnItsForkEvent() {
        // A Session id before the `fork` event (the Origin's, or the new one) is not the end of the setup.
        XCTAssertEqual(StartFlow.outcome(.fork(origin: "s1"), conn(session: "s1")), .waiting)
        XCTAssertEqual(StartFlow.outcome(.fork(origin: "s1"), conn(session: "s2")), .waiting)
        var done = conn(session: "s2")
        done.fork = .init(sessionId: "s2")
        XCTAssertEqual(StartFlow.outcome(.fork(origin: "s1"), done), .ready(sessionId: "s2"))
        var refused = conn(session: nil)
        refused.fork = .init(error: .init(code: "bad_prompt", message: "没有这个提示词"))
        XCTAssertEqual(StartFlow.outcome(.fork(origin: "s1"), refused), .failed("没有这个提示词"))
        // ccrpc ending before the event is a failure too.
        XCTAssertEqual(StartFlow.outcome(.fork(origin: "s1"), conn(session: nil, live: false, stderr: "error: boom")),
                       .failed("error: boom"))
    }

    func testEndingBeforeANameIsAFailureWithItsReason() {
        XCTAssertEqual(StartFlow.outcome(.new(cwd: "/w"), conn(session: nil, live: false, stderr: "x\nerror: no such directory\n")),
                       .failed("error: no such directory"))
        var exited = conn(session: nil)
        exited.sessionExited = true
        XCTAssertEqual(StartFlow.outcome(.resume(sessionId: "s1"), exited), .failed("claude 退出了"))
    }

    // MARK: New Session directories

    func testDirectoriesSidebarFirstThenRecentSkippingMissing() {
        let projects = [
            ProjectSummary(id: "p1", path: "/a", displayName: "a", missing: false, lastActiveMs: 1),
            ProjectSummary(id: "p2", path: "/b", displayName: "b", missing: false, lastActiveMs: 3),
            ProjectSummary(id: "p3", path: "/gone", displayName: "gone", missing: true, lastActiveMs: 9),
        ]
        let dirs = StartFlow.directories(sidebar: [.init(path: "/a"), .init(path: "/side/x")], projects: projects)
        XCTAssertEqual(dirs.map(\.path), ["/a", "/side/x", "/b"])
        XCTAssertEqual(dirs.map(\.name), ["a", "x", "b"])
        XCTAssertEqual(StartFlow.cwd(from: "  /work/app/ "), "/work/app")
        XCTAssertEqual(StartFlow.cwd(from: "/"), "/")
    }

    func testOnlyFullPathsCanStartASession() {
        XCTAssertNil(StartFlow.cwdProblem(""))
        XCTAssertNil(StartFlow.cwdProblem("  /work/app/ "))
        XCTAssertNil(StartFlow.cwdProblem("/"))
        XCTAssertEqual(StartFlow.cwdProblem("~/work/app"), "请输入以 / 开头的完整路径")
        XCTAssertEqual(StartFlow.cwdProblem("work/app"), "请输入以 / 开头的完整路径")
    }
}

@MainActor
final class StartFollowStoreTests: XCTestCase {
    func testFollowingANewSessionThroughStateEvents() {
        let store = ConnectStore()
        store.starts["c1"] = .new(cwd: "/w")
        var c = ConnState(conn: "c1")
        c.state = .starting
        store.apply(c)
        XCTAssertNil(store.started["c1"])
        c.sessionId = "s9"
        c.ready = true
        store.apply(c)
        XCTAssertEqual(store.started["c1"], "s9")
        XCTAssertNil(store.starts["c1"])
    }

    func testAForkOpensItsSessionAndAFailureGoesOnItsOrigin() {
        let store = ConnectStore()
        store.starts["c1"] = .fork(origin: "s1")
        var c = ConnState(conn: "c1")
        c.sessionId = "s2"
        store.apply(c)
        XCTAssertNil(store.openRequest)
        c.fork = .init(sessionId: "s2", titleError: "rename failed")
        store.apply(c)
        XCTAssertEqual(store.openRequest?.sessionId, "s2")
        XCTAssertEqual(store.errors["s2"], "Fork 的标题没有改成：rename failed")

        store.starts["c3"] = .fork(origin: "s1")
        var refused = ConnState(conn: "c3")
        refused.fork = .init(error: .init(code: "bad_prompt", message: "没有这个提示词"))
        store.apply(refused)
        XCTAssertEqual(store.errors["s1"], "Fork 失败：没有这个提示词")

        store.starts["c2"] = .fork(origin: "s1")
        var failed = ConnState(conn: "c2")
        failed.procExit = .init(code: 1, stderr: "error: at 不在 Main Line 上")
        store.apply(failed)
        store.remove(conn: "c2")
        XCTAssertEqual(store.errors["s1"], "Fork 失败：error: at 不在 Main Line 上")
        XCTAssertNil(store.conns["c2"])
    }

    func testAForkWithACardOpensItsSessionOnce() throws {
        let store = ConnectStore()
        store.pageOpened("s1")
        store.starts["c1"] = .fork(origin: "s1")
        var c = ConnState(conn: "c1")
        c.sessionId = "s2"
        store.apply(c)
        XCTAssertNil(store.openRequest)
        c.requests = [try JSONDecoder().decode(
            ConnRequest.self, from: Data(#"{"type": "choice", "request_id": "choice:1", "title": "Trust?", "options": [{"label": "Yes"}]}"#.utf8)
        )]
        store.apply(c)
        XCTAssertEqual(store.openRequest?.sessionId, "s2")
        store.openRequest = nil
        store.apply(c)
        XCTAssertNil(store.openRequest, "once")
        c.fork = .init(sessionId: "s2")
        store.apply(c)
        XCTAssertNil(store.openRequest, "already open")
        XCTAssertEqual(store.started["c1"], "s2")
    }

    func testAForkCardWaitsForItsOriginsPage() throws {
        let store = ConnectStore()
        store.starts["c1"] = .fork(origin: "s1")
        var c = ConnState(conn: "c1")
        c.sessionId = "s2"
        c.requests = [try JSONDecoder().decode(
            ConnRequest.self, from: Data(#"{"type": "choice", "request_id": "choice:1", "title": "Trust?", "options": [{"label": "Yes"}]}"#.utf8)
        )]
        store.apply(c)
        XCTAssertNil(store.openRequest, "its origin is off screen")
        store.pageOpened("s1")
        XCTAssertEqual(store.openRequest?.sessionId, "s2")
    }

    func testAForkFailingAfterACardSaysSoOnItsOwnPageToo() throws {
        let store = ConnectStore()
        store.pageOpened("s1")
        store.starts["c1"] = .fork(origin: "s1")
        var c = ConnState(conn: "c1")
        c.sessionId = "s2"
        c.requests = [try JSONDecoder().decode(
            ConnRequest.self, from: Data(#"{"type": "choice", "request_id": "choice:1", "title": "Trust?", "options": [{"label": "Yes"}]}"#.utf8)
        )]
        store.apply(c)
        XCTAssertEqual(store.openRequest?.sessionId, "s2")
        c.requests = []
        c.sessionExited = true
        store.apply(c)
        store.remove(conn: "c1")
        XCTAssertEqual(store.errors["s1"], store.errors["s2"])
        XCTAssertEqual(store.errors["s2"]?.hasPrefix("Fork 失败："), true)
    }

    func testANewSessionRemovedBeforeItsNameFails() {
        let store = ConnectStore()
        store.starts["c1"] = .new(cwd: "/w")
        store.apply(ConnState(conn: "c1"))
        store.remove(conn: "c1")
        XCTAssertEqual(store.startFailures["c1"], "claude 退出了")
    }
}

/// The Host, as canned answers by RPC method; records what was called (`connect_command:<type>` for commands).
private final class StubHost: URLProtocol, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var answers: [String: String] = [:]
    nonisolated(unsafe) private static var log: [String] = []

    static func set(_ answers: [String: String]) {
        lock.withLock {
            self.answers = answers
            log = []
        }
    }

    static func answer(_ method: String, _ json: String?) { lock.withLock { answers[method] = json } }

    static var calls: [String] { lock.withLock { log } }

    static func client() -> HostClient {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubHost.self]
        return HostClient(address: HostAddress(host: "127.0.0.1", port: 9), session: URLSession(configuration: config))
    }

    override class func canInit(with _: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        var body = request.httpBody ?? Data()
        if let stream = request.httpBodyStream {
            stream.open()
            defer { stream.close() }
            var buffer = [UInt8](repeating: 0, count: 65536)
            while stream.hasBytesAvailable {
                let n = stream.read(&buffer, maxLength: buffer.count)
                if n <= 0 { break }
                body.append(buffer, count: n)
            }
        }
        let rpc = (try? JSONSerialization.jsonObject(with: body)) as? [String: Any]
        let method = rpc?["method"] as? String ?? ""
        let type = ((rpc?["args"] as? [String: Any])?["command"] as? [String: Any])?["type"] as? String
        let reply: String? = Self.lock.withLock {
            Self.log.append(type.map { "\(method):\($0)" } ?? method)
            return Self.answers[method]
        }
        guard let reply else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(reply.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
}

/// Resume and failed starts against a stubbed Host.
@MainActor
final class ResumeAgainstHostTests: XCTestCase {
    private let starting = #"{"result": [{"conn": "c1", "state": "starting"}]}"#
    private let named = #"{"result": [{"conn": "c1", "sessionId": "s1", "ready": true, "state": "idle"}]}"#
    private let commandOK = #"{"result": {"success": true, "data": {"lines": ["claude 没起来", ""], "cursor": [1, 0]}}}"#

    private func store(timeout: Duration = .milliseconds(20)) -> ConnectStore {
        let store = ConnectStore()
        store.reset(client: StubHost.client())
        store.resumeTimeout = timeout
        return store
    }

    private func waitUntil(_ what: String, _ condition: () -> Bool) async throws {
        for _ in 0..<300 {
            if condition() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("timed out: \(what)")
    }

    func testASecondTapStartsNothing() async {
        StubHost.set(["ccrpc_resume": #"{"result": "c1"}"#, "connect_states": starting])
        let store = store(timeout: .seconds(60))
        async let first: Void = store.resume(sessionId: "s1", cwd: "/w")
        async let second: Void = store.resume(sessionId: "s1", cwd: "/w")
        _ = await (first, second)
        XCTAssertEqual(StubHost.calls.filter { $0 == "ccrpc_resume" }.count, 1)
        XCTAssertTrue(store.isResuming("s1"))
    }

    func testAResumeThatCouldNotStartCanBeTriedAgain() async {
        StubHost.set(["ccrpc_resume": #"{"error": {"code": "x", "message": "没有 ccrpc"}}"#])
        let store = store(timeout: .seconds(60))
        await store.resume(sessionId: "s1", cwd: "/w")
        XCTAssertFalse(store.isResuming("s1"))
        XCTAssertEqual(store.errors["s1"], "Resume 失败：没有 ccrpc")
    }

    func testAForkIsMarkedBeforeTheHostAnswers() async {
        StubHost.set(["ccrpc_fork": #"{"result": "c1"}"#])
        let store = store(timeout: .seconds(60))
        async let first: Void = store.fork(sessionId: "s1", cwd: "/w", title: "t (Fork)")
        async let second: Void = store.fork(sessionId: "s1", cwd: "/w", title: "t (Fork)")
        _ = await (first, second)
        XCTAssertEqual(StubHost.calls.filter { $0 == "ccrpc_fork" }.count, 1)
    }

    func testTheTimeoutGivesUpOnlyOnWhatTheHostSaysNow() async throws {
        StubHost.set([
            "ccrpc_resume": #"{"result": "c1"}"#, "connect_states": starting, "connect_command": commandOK,
            "ccrpc_disconnect": #"{"result": null}"#,
        ])
        let store = store()
        await store.resume(sessionId: "s1", cwd: "/w")
        try await waitUntil("gave up") { !store.isResuming("s1") }
        XCTAssertTrue(StubHost.calls.contains("connect_command:exit"))
        XCTAssertEqual(store.errors["s1"], "Resume 失败：30 秒内没有启动（claude 没起来）")
    }

    func testReadyWithoutANameIsNotStarted() async throws {
        StubHost.set([
            "ccrpc_resume": #"{"result": "c1"}"#, "connect_states": #"{"result": [{"conn": "c1", "ready": true, "state": "idle"}]}"#,
            "connect_command": commandOK, "ccrpc_disconnect": #"{"result": null}"#,
        ])
        let store = store()
        await store.resume(sessionId: "s1", cwd: "/w")
        try await waitUntil("gave up") { !store.isResuming("s1") }
        XCTAssertTrue(StubHost.calls.contains("connect_command:exit"))
    }

    func testASessionThatStartedMeanwhileIsLeftAlone() async throws {
        // The app was suspended: no event told it the Session started, only the Host's state now does.
        StubHost.set(["ccrpc_resume": #"{"result": "c1"}"#, "connect_states": named, "connect_command": commandOK])
        let store = store()
        await store.resume(sessionId: "s1", cwd: "/w")
        try await waitUntil("followed") { !store.isResuming("s1") }
        try await Task.sleep(for: .milliseconds(60))
        XCTAssertFalse(StubHost.calls.contains { $0.hasPrefix("connect_command") })
        XCTAssertNil(store.errors["s1"])
        XCTAssertEqual(store.started["c1"], "s1")
    }

    func testAHostThatCannotBeAskedProvesNothing() async throws {
        StubHost.set(["ccrpc_resume": #"{"result": "c1"}"#, "connect_command": commandOK])
        let store = store()
        await store.resume(sessionId: "s1", cwd: "/w")
        try await waitUntil("asked twice") { StubHost.calls.filter { $0 == "connect_states" }.count >= 2 }
        XCTAssertFalse(StubHost.calls.contains { $0.hasPrefix("connect_command") })
        XCTAssertTrue(store.isResuming("s1"))
        store.reset(client: nil)
    }

    func testNamedDuringTheLastMomentsNothingIsEnded() async throws {
        StubHost.set(["ccrpc_resume": #"{"result": "c1"}"#, "connect_states": starting, "connect_command": commandOK])
        let store = store()
        await store.resume(sessionId: "s1", cwd: "/w")
        try await waitUntil("giving up") { store.failingResumes.contains("c1") }
        var ready = ConnState(conn: "c1")
        ready.sessionId = "s1"
        ready.ready = true
        store.apply(ready)
        try await waitUntil("stopped") { !store.failingResumes.contains("c1") }
        XCTAssertFalse(StubHost.calls.contains("connect_command:exit"))
        XCTAssertNil(store.errors["s1"])
    }

    func testACardKeepsTheTimeoutWaiting() async throws {
        let card = #"{"result": [{"conn": "c1", "state": "starting", "requests": [{"type": "choice", "request_id": "choice:1", "title": "Trust?", "options": [{"label": "Yes"}]}]}]}"#
        StubHost.set(["ccrpc_resume": #"{"result": "c1"}"#, "connect_states": card, "connect_command": commandOK])
        let store = store()
        await store.resume(sessionId: "s1", cwd: "/w")
        try await waitUntil("asked twice") { StubHost.calls.filter { $0 == "connect_states" }.count >= 2 }
        XCTAssertFalse(StubHost.calls.contains { $0.hasPrefix("connect_command") })
        XCTAssertEqual(store.conn(for: "s1")?.requests.count, 1, "the card is on the Session's page")
        store.reset(client: nil)
    }

    func testAnAnsweredCardGivesClaudeTheWholeTimeout() async throws {
        // Right after the answer the Host has neither a card nor a name.
        StubHost.set([
            "ccrpc_resume": #"{"result": "c1"}"#, "connect_states": starting, "connect_command": commandOK,
            "ccrpc_disconnect": #"{"result": null}"#,
        ])
        let store = store(timeout: .milliseconds(400))
        await store.resume(sessionId: "s1", cwd: "/w")
        var c = ConnState(conn: "c1")
        c.state = .starting
        c.requests = [try JSONDecoder().decode(
            ConnRequest.self, from: Data(#"{"type": "choice", "request_id": "choice:1", "title": "Trust?", "options": [{"label": "Yes"}]}"#.utf8)
        )]
        store.apply(c)
        try await Task.sleep(for: .milliseconds(300))
        c.requests = []
        store.apply(c)
        try await waitUntil("the timer ran out") { StubHost.calls.contains("connect_states") }
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertFalse(StubHost.calls.contains { $0.hasPrefix("connect_command") })
        XCTAssertTrue(store.isResuming("s1"))
        try await waitUntil("gave up") { !store.isResuming("s1") }
        XCTAssertTrue(StubHost.calls.contains("connect_command:exit"))
    }

    func testANewSessionThatFailedEndsItsCcrpc() async throws {
        StubHost.set(["ccrpc_disconnect": #"{"result": null}"#])
        let store = store()
        store.starts["c1"] = .new(cwd: "/w")
        var exited = ConnState(conn: "c1")
        exited.sessionExited = true
        store.apply(exited)
        try await waitUntil("disconnected") { StubHost.calls.contains("ccrpc_disconnect") }
        XCTAssertEqual(store.startFailures["c1"], "claude 退出了")
    }

    func testASnapshotKeepsWhatEndedByItself() async {
        StubHost.set(["connect_states": #"{"result": []}"#])
        let store = store()
        var crashed = ConnState(conn: "c1")
        crashed.sessionId = "s1"
        crashed.procExit = .init(code: 1, stderr: "boom")
        store.apply(crashed)
        store.remove(conn: "c1")
        await store.syncStates()
        XCTAssertEqual(store.conn(for: "s1")?.procExit?.code, 1)
    }
}
