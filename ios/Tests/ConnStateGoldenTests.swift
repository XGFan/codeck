import XCTest
@testable import CoDeck

/// `Fixtures/conn-state.json` is what the Host's Rust code produces for a scripted ccrpc session
/// (`UPDATE_GOLDEN=1 cargo test -p codeck-host --test conn_state_golden` rewrites it): both sides decode one file.
final class ConnStateGoldenTests: XCTestCase {
    private func golden() throws -> ConnState {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "conn-state", withExtension: "json"))
        return try JSONDecoder().decode(ConnState.self, from: Data(contentsOf: url))
    }

    func testDecodesWhatTheHostProduces() throws {
        let c = try golden()
        XCTAssertEqual(c.conn, "c1")
        XCTAssertEqual(c.state, .waiting)
        XCTAssertEqual(c.waiting?.kind, "permission")
        XCTAssertEqual(c.mode, "plan")
        XCTAssertEqual(c.effort, "xhigh")
        XCTAssertEqual(c.contextTokens, 52000)

        XCTAssertEqual(c.requests.map(\.id), ["toolu_1", "toolu_2", "rewind:1"])
        guard case .permission(let p) = c.requests[0] else { return XCTFail("permission_request") }
        XCTAssertEqual(p.requestId, "toolu_1")
        XCTAssertEqual(p.toolUseId, "toolu_1")
        XCTAssertNotNil(p.allowAlways)
        guard case .question(let q) = c.requests[1] else { return XCTFail("question") }
        XCTAssertEqual(q.requestId, "toolu_2")
        XCTAssertEqual(q.toolUseId, "toolu_2")
        XCTAssertEqual(q.agentId, "agent-7")
        guard case .rewind(let rw) = c.requests[2] else { return XCTFail("rewind") }
        XCTAssertEqual(rw.code, .init(restores: true, delta: "-1", target: "w.txt"))
        XCTAssertEqual(rw.codeText, "The code will be restored -1 in w.txt.")
        XCTAssertEqual(rw.options, [.both, .conversation])
        XCTAssertEqual(RewindWords.codeLine(rw.code, rw.codeText), "这条提示词及之后的对话会被移除；代码会恢复：w.txt（−1）")

        XCTAssertEqual(c.queue.map(\.kind), ["typed", "session"])
        XCTAssertEqual(c.queue.map(\.editable), [true, false])
        XCTAssertEqual(c.queue.map(\.label), ["and then run lint", "api：migration is done"])
        XCTAssertEqual(c.tasks.count, 1)
        XCTAssertEqual(c.tasks.first?.id, "b1")
        XCTAssertEqual(c.tasks.first?.kind, "shell")
        XCTAssertEqual(c.tasks.first?.status, "running")
        XCTAssertEqual(c.tasks.first?.toolUseId, "toolu_9")
        XCTAssertEqual(c.tasks.first?.idle, false)
        XCTAssertEqual(c.lastTurn?.durationMs, 8123)
        XCTAssertEqual(c.notices.count, 1)
        XCTAssertTrue(c.notices[0].text.hasPrefix("排队的消息没有发出"))
        XCTAssertNil(c.procExit)
        XCTAssertFalse(c.sessionExited)
        XCTAssertTrue(c.canCommand)
    }
}
