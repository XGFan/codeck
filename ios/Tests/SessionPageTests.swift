import XCTest
@testable import CoDeck

final class SessionPageTests: XCTestCase {
    func testComposerButton() {
        func action(_ state: ConnState.Phase?, content: Bool = false, uploading: Bool = false) -> ComposerAction {
            var c = ConnState(conn: "c")
            c.state = state
            return .of(state: state, isBusy: c.isBusy, hasContent: content, uploading: uploading)
        }
        XCTAssertEqual(action(.busy), .stop)
        XCTAssertEqual(action(.waiting), .stop)
        XCTAssertEqual(action(.busy, content: true), .steer)
        // A photo still on its way counts as something to send, not as an empty box to stop from.
        XCTAssertEqual(action(.busy, uploading: true), .steer)
        XCTAssertEqual(action(.waiting, content: true), .send)
        XCTAssertEqual(action(.idle), .send)
    }

    @MainActor
    func testControlRowHidesOnlyWhenNothingNeedsIt() {
        func shows(
            _ state: ConnState.Phase? = .idle, atBottom: Bool = false, focused: Bool = false,
            error: Bool = false, queue: Bool = false
        ) -> Bool {
            ConnectedBar.showsControls(state: state, atBottom: atBottom, focused: focused, hasError: error, hasQueue: queue)
        }
        XCTAssertFalse(shows())
        XCTAssertTrue(shows(atBottom: true))
        XCTAssertTrue(shows(focused: true))
        XCTAssertTrue(shows(error: true))
        XCTAssertTrue(shows(queue: true))
        XCTAssertFalse(shows(.waiting, atBottom: true))
    }

    func testHeaderStatusSaysWhatTheConnectIsDoing() {
        var c = ConnState(conn: "c")
        c.state = .busy
        c.progress = ConnState.Progress(text: "运行 pnpm test", verb: "Running", elapsedS: 92, tokens: nil)
        XCTAssertEqual(HeaderStatus.connected(c), HeaderStatus(text: "工作中 · 运行 pnpm test", dot: Palette.ok, elapsed: "1 分 32 秒"))

        c.progress = ConnState.Progress(text: nil, verb: "Thinking", elapsedS: nil, tokens: nil)
        XCTAssertEqual(HeaderStatus.connected(c).text, "工作中 · Thinking")

        // Only a working Connect says what it does; what is left from the last turn is not news.
        c.state = .waiting
        XCTAssertEqual(HeaderStatus.connected(c), HeaderStatus(text: "等待作答", dot: Palette.warn))
    }
}
