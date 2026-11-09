import XCTest
@testable import CoDeck

final class HostAddressTests: XCTestCase {
    private func parse(_ host: String, _ port: String = "38471", loopback: Bool = false) throws -> HostAddress {
        try HostAddress.parse(host: host, port: port, allowLoopback: loopback)
    }

    func testAcceptsPrivateRanges() throws {
        for host in ["10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.254", "192.168.1.20"] {
            XCTAssertEqual(try parse(host).host, host)
        }
    }

    func testRejectsPublicAndBoundaryAddresses() {
        for host in ["8.8.8.8", "172.15.0.1", "172.32.0.1", "192.169.0.1", "11.0.0.1", "0.0.0.0"] {
            XCTAssertThrowsError(try parse(host), host) { XCTAssertEqual($0 as? HostAddress.ValidationError, .notPrivate) }
        }
    }

    func testRejectsGarbage() {
        for host in ["", "abc", "192.168.1", "192.168.1.256", "192.168.1.1.1", "192.168..1", "192.168.1.-1", "١٩٢.168.1.1", "mac.local"] {
            XCTAssertThrowsError(try parse(host), host) { XCTAssertEqual($0 as? HostAddress.ValidationError, .notIPv4) }
        }
    }

    func testPortHandling() throws {
        XCTAssertEqual(try parse("192.168.1.20", "").port, HostAddress.defaultPort)
        XCTAssertEqual(try parse("192.168.1.20", " 8080 ").port, 8080)
        XCTAssertEqual(try parse("http://192.168.1.20:9000/", "38471"), HostAddress(host: "192.168.1.20", port: 9000))
        for port in ["0", "65536", "-1", "80a", "1.5"] {
            XCTAssertThrowsError(try parse("192.168.1.20", port), port) {
                XCTAssertEqual($0 as? HostAddress.ValidationError, .badPort)
            }
        }
    }

    func testNormalizesLeadingZeros() throws {
        XCTAssertEqual(try parse("192.168.001.020").host, "192.168.1.20")
    }

    func testLoopbackOnlyWhenAllowed() throws {
        XCTAssertThrowsError(try parse("127.0.0.1"))
        XCTAssertEqual(try parse("127.0.0.1", loopback: true).host, "127.0.0.1")
        XCTAssertThrowsError(try parse("127.0.0.2", loopback: true))
    }

    func testStoreRoundTrip() throws {
        let defaults = try XCTUnwrap(UserDefaults(suiteName: "HostAddressTests"))
        defaults.removePersistentDomain(forName: "HostAddressTests")
        let store = HostAddressStore(defaults: defaults)
        XCTAssertNil(store.load())
        store.save(HostAddress(host: "10.1.2.3", port: 4000))
        XCTAssertEqual(store.load(), HostAddress(host: "10.1.2.3", port: 4000))
    }
}
