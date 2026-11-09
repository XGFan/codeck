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

    func testAForkIsReadyOnlyWithAnIdOtherThanItsOrigin() {
        XCTAssertEqual(StartFlow.outcome(.fork(origin: "s1"), conn(session: "s1")), .waiting)
        XCTAssertEqual(StartFlow.outcome(.fork(origin: "s1"), conn(session: "s2")), .ready(sessionId: "s2"))
        XCTAssertEqual(StartFlow.outcome(.fork(origin: "s1"), conn(session: "s1", live: false, notice: "Fork 失败：没有这个提示词")),
                       .failed("Fork 失败：没有这个提示词"))
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
        XCTAssertEqual(store.openRequest?.sessionId, "s2")

        store.starts["c2"] = .fork(origin: "s1")
        var failed = ConnState(conn: "c2")
        failed.procExit = .init(code: 1, stderr: "error: at 不在 Main Line 上")
        store.apply(failed)
        store.remove(conn: "c2")
        XCTAssertEqual(store.errors["s1"], "Fork 失败：error: at 不在 Main Line 上")
        XCTAssertNil(store.conns["c2"])
    }

    func testANewSessionRemovedBeforeItsNameFails() {
        let store = ConnectStore()
        store.starts["c1"] = .new(cwd: "/w")
        store.apply(ConnState(conn: "c1"))
        store.remove(conn: "c1")
        XCTAssertEqual(store.startFailures["c1"], "claude 退出了")
    }
}
