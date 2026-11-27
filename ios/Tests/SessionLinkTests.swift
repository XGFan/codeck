import XCTest
@testable import CoDeck

final class SessionLinkTests: XCTestCase {
    private let id = "3f2b6c1e-8a4d-4e5f-9b7a-0c1d2e3f4a5b"

    func testParsesUUIDCaseInsensitivelyAndIgnoresQuery() {
        XCTAssertEqual(SessionLink.parse(URL(string: "codeck://session/\(id)")!), id)
        XCTAssertEqual(SessionLink.parse(URL(string: "codeck://session/\(id.uppercased())?from=bark")!), id)
        XCTAssertEqual(SessionLink.parse(URL(string: "CODECK://Session/\(id)")!), id)
    }

    func testRejectsOtherLinks() {
        for bad in [
            "codeck://session/not-a-uuid", "codeck://session/", "codeck://session", "codeck://session/\(id)/x",
            "codeck://pair/\(id)", "https://session/\(id)", "codeck://session:80/\(id)",
        ] {
            XCTAssertNil(SessionLink.parse(URL(string: bad)!), bad)
        }
    }
}
