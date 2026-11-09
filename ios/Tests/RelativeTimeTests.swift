import XCTest
@testable import CoDeck


final class RelativeTimeTests: XCTestCase {
    func testRelativeTime() {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "Asia/Shanghai")!
        let now = cal.date(from: DateComponents(year: 2026, month: 10, day: 7, hour: 12))!
        func at(_ d: Int, _ h: Int, _ m: Int = 0, _ s: Int = 0) -> Date {
            cal.date(from: DateComponents(year: 2026, month: 10, day: d, hour: h, minute: m, second: s))!
        }
        XCTAssertEqual(RelativeTime.format(at(7, 11, 59, 40), now: now, calendar: cal), "刚刚")
        XCTAssertEqual(RelativeTime.format(at(7, 11, 54), now: now, calendar: cal), "6 分钟前")
        XCTAssertEqual(RelativeTime.format(at(7, 1), now: now, calendar: cal), "11 小时前")
        XCTAssertEqual(RelativeTime.format(at(6, 23), now: now, calendar: cal), "昨天")
        XCTAssertEqual(RelativeTime.format(at(4, 9), now: now, calendar: cal), "周日")
        XCTAssertEqual(RelativeTime.format(at(1, 9), now: now, calendar: cal), "周四")
        let lastMonth = cal.date(from: DateComponents(year: 2026, month: 9, day: 30, hour: 9))!
        XCTAssertEqual(RelativeTime.format(lastMonth, now: now, calendar: cal), "9月30日")
    }
}
