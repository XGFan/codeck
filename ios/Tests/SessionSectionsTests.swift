import XCTest
@testable import CoDeck

final class SessionSectionsTests: XCTestCase {
    private func session(_ id: String, _ status: LiveStatus?, at ms: Double) -> SessionSummary {
        SessionSummary(
            id: id, projectId: "p", title: id, createdMs: 0, lastActiveMs: ms, messageCount: 1,
            live: status.map { LiveState(status: $0) }
        )
    }

    func testSections() {
        let all = [
            session("old", nil, at: 1),
            session("idle", .idle, at: 5),
            session("busy-old", .busy, at: 2),
            session("busy-new", .busy, at: 6),
            session("waiting", .waiting, at: 3),
            session("asks", .busy, at: 4),  // requests from a Connect before Live says waiting
            session("ended-asks", nil, at: 7),  // no Live state, but its Connect has requests
            session("recent", nil, at: 8),
        ]
        let s = SessionSections(all) { ["asks", "ended-asks"].contains($0) }
        XCTAssertEqual(s.pending.map(\.id), ["ended-asks", "asks", "waiting"])
        XCTAssertEqual(s.running.map(\.id), ["busy-new", "busy-old", "idle"])
        XCTAssertEqual(s.recent.map(\.id), ["recent", "old"])
        XCTAssertEqual(s.pending.count + s.running.count + s.recent.count, all.count)
    }

    /// Cards answered in place do not move: what is already shown keeps its order, and a newcomer goes last.
    func testPendingKeepsItsOrder() {
        let all = [session("a", .waiting, at: 1), session("b", .waiting, at: 9), session("new", .waiting, at: 20)]
        let s = SessionSections(all, pendingOrder: ["a", "b", "gone"]) { _ in false }
        XCTAssertEqual(s.pending.map(\.id), ["a", "b", "new"])
    }

    func testWaitingWithoutConnectIsPending() {
        let s = SessionSections([session("w", .waiting, at: 1)]) { _ in false }
        XCTAssertEqual(s.pending.map(\.id), ["w"])
        XCTAssertTrue(s.running.isEmpty)
    }
}
