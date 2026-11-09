import XCTest
@testable import CoDeck

/// JSON shaped like src/ipc/bindings.ts (serde camelCase), including fields the app ignores.
final class DecodingTests: XCTestCase {
    private let sessionJSON = """
    {
      "id": "7c1e", "projectId": "p1", "title": "修复侧栏滚动", "titleSource": "ai",
      "createdMs": 1762650000000, "lastActiveMs": 1762653600000, "lastPromptMs": 1762653000000,
      "messageCount": 42, "toolCallCount": 10, "subagentCount": 1,
      "tokens": {"input": 1, "output": 2, "cacheRead": 3, "cacheWrite": 4},
      "cost": 0.12, "unpricedMessages": 0, "gitBranch": null,
      "live": {"status": "waiting", "rawStatus": "waiting", "waitingFor": "permission prompt", "pid": 123},
      "forkOrigin": null, "primaryModel": "claude-opus-5-5", "editedProjectIds": ["p1"]
    }
    """

    func testSessionSummary() throws {
        let s = try JSONDecoder().decode(SessionSummary.self, from: Data(sessionJSON.utf8))
        XCTAssertEqual(s.id, "7c1e")
        XCTAssertEqual(s.title, "修复侧栏滚动")
        XCTAssertEqual(s.messageCount, 42)
        XCTAssertNil(s.gitBranch)
        XCTAssertEqual(s.live?.status, .waiting)
        XCTAssertEqual(s.live?.waitingFor, "permission prompt")
        XCTAssertEqual(s.lastActive, Date(timeIntervalSince1970: 1_762_653_600))
    }

    func testRPCEnvelopeWithSessionList() throws {
        let body = Data(#"{"result": [\#(sessionJSON)]}"#.utf8)
        let list: [SessionSummary] = try HostClient.decodeRPC(body, status: 200)
        XCTAssertEqual(list.map(\.id), ["7c1e"])
    }

    func testRPCErrorCarriesMessage() {
        let body = Data(#"{"error": {"code": "notFound", "message": "找不到这个 Session。"}}"#.utf8)
        XCTAssertThrowsError(try HostClient.decodeRPC(body, status: 404) as [SessionSummary]) {
            XCTAssertEqual($0 as? HostError, .host(code: "notFound", message: "找不到这个 Session。"))
            XCTAssertEqual(($0 as? HostError)?.errorDescription, "找不到这个 Session。")
        }
        XCTAssertThrowsError(try HostClient.decodeRPC(Data("oops".utf8), status: 500) as [SessionSummary]) {
            XCTAssertEqual($0 as? HostError, .badResponse)
        }
    }

    func testProjectSummary() throws {
        let json = """
        [{"id": "p1", "path": "/Users/a/codeck", "displayName": "codeck", "missing": false,
          "sessionCount": 12, "lastActiveMs": 1762653600000}]
        """
        let p = try JSONDecoder().decode([ProjectSummary].self, from: Data(json.utf8))
        XCTAssertEqual(p.first?.displayName, "codeck")
        XCTAssertEqual(p.first?.sessionCount, 12)
    }

    func testEvents() throws {
        let live = #"{"event": "live-changed-event", "payload": {"live": [{"sessionId": "a", "state": {"status": "busy", "rawStatus": null, "waitingFor": null, "pid": null}}, {"sessionId": "b", "state": {"status": "somethingNew", "rawStatus": "x", "waitingFor": null, "pid": 1}}]}}"#
        guard case .liveChanged(let snapshot) = try HostEvent.decode(Data(live.utf8)) else { return XCTFail() }
        XCTAssertEqual(snapshot.live.map(\.state.status), [.busy, .unknown])

        let changed = #"{"event": "sessions-changed-event", "payload": {"changed": ["a"], "removed": [], "projectsChanged": true}}"#
        guard case .sessionsChanged(let c) = try HostEvent.decode(Data(changed.utf8)) else { return XCTFail() }
        XCTAssertEqual(c.changed, ["a"])
        XCTAssertTrue(c.projectsChanged)

        guard case .hello(1) = try HostEvent.decode(Data(#"{"event": "hello", "payload": {"apiVersion": 1}}"#.utf8)) else {
            return XCTFail()
        }
        guard case .pricesChanged = try HostEvent.decode(Data(#"{"event": "prices-changed-event", "payload": null}"#.utf8)) else {
            return XCTFail()
        }
        guard case .other("connect-state") = try HostEvent.decode(Data(#"{"event": "connect-state", "payload": {}}"#.utf8)) else {
            return XCTFail()
        }
    }

    func testSessionQueryEncodesExplicitNulls() throws {
        let data = try JSONEncoder().encode(SessionQuery.all)
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["projectIds"] as? [String], [])
        XCTAssertEqual(obj["sort"] as? String, "lastActive")
        XCTAssertEqual(obj["descending"] as? Bool, true)
        XCTAssertTrue(obj["timeRange"] is NSNull)
        XCTAssertTrue(obj["drill"] is NSNull)
    }

    func testHealthVersionCheck() throws {
        XCTAssertEqual(try HostClient.validateHealth(Data(#"{"product":"CoDeck","apiVersion":1,"version":"0.9.0"}"#.utf8)).version, "0.9.0")
        XCTAssertThrowsError(try HostClient.validateHealth(Data(#"{"product":"CoDeck","apiVersion":2,"version":"1.0.0"}"#.utf8))) {
            XCTAssertEqual($0 as? HostError, .versionMismatch(host: 2))
        }
        XCTAssertThrowsError(try HostClient.validateHealth(Data(#"{"product":"Other","apiVersion":1}"#.utf8))) {
            XCTAssertEqual($0 as? HostError, .notCoDeck)
        }
        XCTAssertThrowsError(try HostClient.validateHealth(Data("<html>".utf8))) {
            XCTAssertEqual($0 as? HostError, .notCoDeck)
        }
    }
}
