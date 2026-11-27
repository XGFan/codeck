import Foundation
import XCTest
@testable import CoDeck

final class PairingTests: XCTestCase {
    private static let validFP = "vu8xtlO5XgT96RXqPEF3L6izmPVudCDb-uomlgZtiBQ"
    private static let otherFP = "6o220Q-JEKn5p9inkMLss_p7OlCPpZlM1NMxXHLQLh8"
    private static let validKey = "AbCdEf0123456789-_xyZw"

    // MARK: PairingLink

    func testPairingLinkSingleAndMultiAddress() throws {
        let single = try PairingLink.parse(
            "codeck://pair?h=192.168.1.20&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"
        )
        XCTAssertEqual(single.hosts, ["192.168.1.20"])
        XCTAssertEqual(single.port, 38471)
        XCTAssertEqual(single.fingerprint, Self.validFP)
        XCTAssertEqual(single.key, Self.validKey)

        let multi = try PairingLink.parse(
            URL(string: "codeck://pair?h=192.168.001.020,10.0.0.5,172.16.4.2&p=443&fp=\(Self.validFP)&k=\(Self.validKey)")!
        )
        XCTAssertEqual(multi.hosts, ["192.168.1.20", "10.0.0.5", "172.16.4.2"])
        XCTAssertEqual(multi.port, 443)
        XCTAssertEqual(multi.addresses, [
            HostAddress(host: "192.168.1.20", port: 443),
            HostAddress(host: "10.0.0.5", port: 443),
            HostAddress(host: "172.16.4.2", port: 443),
        ])

        let four = try PairingLink.parse(
            "codeck://pair?h=192.168.1.20,10.0.0.5,172.16.4.2,10.0.0.9&p=443&fp=\(Self.validFP)&k=\(Self.validKey)"
        )
        XCTAssertEqual(four.hosts, ["192.168.1.20", "10.0.0.5", "172.16.4.2", "10.0.0.9"])
    }

    func testPairingLinkRejectsInvalidInputs() {
        let valid = "codeck://pair?h=192.168.1.20&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"
        XCTAssertNoThrow(try PairingLink.parse(valid))

        let badCases: [(String, String)] = [
            // Wrong scheme / host / path
            ("wrong scheme", "https://pair?h=192.168.1.20&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("wrong host", "codeck://connect?h=192.168.1.20&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("extra path", "codeck://pair/v1?h=192.168.1.20&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            // Missing required params
            ("missing h", "codeck://pair?p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("missing p", "codeck://pair?h=192.168.1.20&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("missing fp", "codeck://pair?h=192.168.1.20&p=38471&k=\(Self.validKey)"),
            ("missing k", "codeck://pair?h=192.168.1.20&p=38471&fp=\(Self.validFP)"),
            // Duplicate required params
            ("dup h", "codeck://pair?h=10.0.0.1&h=10.0.0.2&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("dup p", "codeck://pair?h=10.0.0.1&p=38471&p=38472&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("dup fp", "codeck://pair?h=10.0.0.1&p=38471&fp=\(Self.validFP)&fp=\(Self.otherFP)&k=\(Self.validKey)"),
            ("dup k", "codeck://pair?h=10.0.0.1&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)&k=\(Self.validKey)"),
            // Non-private addresses (including loopback even in DEBUG)
            ("public IP", "codeck://pair?h=8.8.8.8&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("loopback", "codeck://pair?h=127.0.0.1&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("mixed private+public", "codeck://pair?h=10.0.0.1,8.8.8.8&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("five addresses exceeds limit", "codeck://pair?h=10.0.0.1,10.0.0.2,10.0.0.3,10.0.0.4,10.0.0.5&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("trailing comma", "codeck://pair?h=10.0.0.1,&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("hostname", "codeck://pair?h=mac.local&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)"),
            // Port out of bounds / invalid
            ("port 0", "codeck://pair?h=10.0.0.1&p=0&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("port 65536", "codeck://pair?h=10.0.0.1&p=65536&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("port negative", "codeck://pair?h=10.0.0.1&p=-1&fp=\(Self.validFP)&k=\(Self.validKey)"),
            ("port non-numeric", "codeck://pair?h=10.0.0.1&p=abc&fp=\(Self.validFP)&k=\(Self.validKey)"),
            // Fingerprint length / charset
            ("fp too short", "codeck://pair?h=10.0.0.1&p=38471&fp=\(String(Self.validFP.dropLast()))&k=\(Self.validKey)"),
            ("fp too long", "codeck://pair?h=10.0.0.1&p=38471&fp=\(Self.validFP)A&k=\(Self.validKey)"),
            ("fp padded =", "codeck://pair?h=10.0.0.1&p=38471&fp=\(String(Self.validFP.dropLast()))=&k=\(Self.validKey)"),
            ("fp standard +", "codeck://pair?h=10.0.0.1&p=38471&fp=\(String(Self.validFP.dropLast()))+&k=\(Self.validKey)"),
            ("fp standard /", "codeck://pair?h=10.0.0.1&p=38471&fp=\(String(Self.validFP.dropLast()))/&k=\(Self.validKey)"),
            // Key length / charset
            ("k too short", "codeck://pair?h=10.0.0.1&p=38471&fp=\(Self.validFP)&k=\(String(Self.validKey.dropLast()))"),
            ("k too long", "codeck://pair?h=10.0.0.1&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)A"),
            ("k padded =", "codeck://pair?h=10.0.0.1&p=38471&fp=\(Self.validFP)&k=\(String(Self.validKey.dropLast()))="),
            ("k bad char +", "codeck://pair?h=10.0.0.1&p=38471&fp=\(Self.validFP)&k=\(String(Self.validKey.dropLast()))+"),
        ]

        for (label, raw) in badCases {
            XCTAssertThrowsError(try PairingLink.parse(raw), label) { error in
                XCTAssertEqual(error as? PairingLink.ParseError, .invalid, label)
                XCTAssertEqual((error as? LocalizedError)?.errorDescription, "这不是 CoDeck 的配对二维码。", label)
            }
        }
    }

    // MARK: Fingerprint & PinnedSession

    func testFingerprintMatchesOpenSSLGoldenVectors() {
        // Derived via:
        // printf '\x30\x0d\x06\x09\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0b\x05\x00' | openssl dgst -sha256 -binary | python3 -c "import sys,base64; print(base64.urlsafe_b64encode(sys.stdin.buffer.read()).rstrip(b'=').decode())"
        let asn1DER = Data([0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b, 0x05, 0x00])
        XCTAssertEqual(CertificatePinning.fingerprint(of: asn1DER), Self.validFP)

        // Derived via:
        // printf 'CoDeck-test-DER-bytes' | openssl dgst -sha256 -binary | python3 -c "import sys,base64; print(base64.urlsafe_b64encode(sys.stdin.buffer.read()).rstrip(b'=').decode())"
        // Exercises both '-' and '_' URL-safe substitutions with no '=' padding.
        let sampleDER = Data("CoDeck-test-DER-bytes".utf8)
        XCTAssertEqual(CertificatePinning.fingerprint(of: sampleDER), Self.otherFP)
        XCTAssertEqual(CertificatePinning.fingerprint(of: sampleDER).count, 43)
    }

    func testPinnedSessionConfigurationAndTaskLevelDelegate() {
        let pinned = PinnedSession(expectedFingerprint: Self.validFP)
        XCTAssertNil(pinned.session.configuration.urlCache)

        // Must only implement task-level challenge delegate, NOT session-level.
        XCTAssertFalse(
            pinned.delegate.responds(
                to: #selector(URLSessionDelegate.urlSession(_:didReceive:completionHandler:))
            )
        )
        XCTAssertTrue(
            pinned.delegate.responds(
                to: #selector(URLSessionTaskDelegate.urlSession(_:task:didReceive:completionHandler:))
            )
        )
        XCTAssertTrue(
            pinned.delegate.responds(
                to: #selector(URLSessionTaskDelegate.urlSession(_:task:willPerformHTTPRedirection:newRequest:completionHandler:))
            )
        )

        // Per-task mismatch tracking isolates task identifiers.
        pinned.delegate.recordMismatch(taskIdentifier: 11)
        pinned.delegate.recordMismatch(taskIdentifier: 22)
        XCTAssertFalse(pinned.consumeMismatch(taskIdentifier: 99))
        XCTAssertTrue(pinned.consumeMismatch(taskIdentifier: 11))
        XCTAssertFalse(pinned.consumeMismatch(taskIdentifier: 11))
        XCTAssertTrue(pinned.consumeMismatch(taskIdentifier: 22))
    }

    // MARK: HostStore

    func testHostStoreDeletesLegacyKeysOnInit() throws {
        let suite = "PairingTests.legacy.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        defaults.set("192.168.1.20", forKey: HostStore.legacyHostKey)
        defaults.set(38471, forKey: HostStore.legacyPortKey)

        let store = HostStore(defaults: defaults, keychain: MemoryKeychain(), allowLoopback: false)
        XCTAssertNil(defaults.object(forKey: HostStore.legacyHostKey))
        XCTAssertNil(defaults.object(forKey: HostStore.legacyPortKey))
        XCTAssertNil(store.load())
    }

    func testHostStoreRoundTripAndMissingTokenReturnsNil() throws {
        let suite = "PairingTests.roundtrip.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = MemoryKeychain()
        let store = HostStore(defaults: defaults, keychain: keychain, allowLoopback: false)

        let host = PairedHost(
            address: HostAddress(host: "192.168.1.20", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "a1b2c3d4e5f6",
            token: "tok-123"
        )
        try store.save(host)
        XCTAssertEqual(store.load(), host)
        XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "a1b2c3d4e5f6"), "tok-123")

        // UserDefaults JSON does not contain the token.
        let rawJSON = try XCTUnwrap(defaults.data(forKey: HostStore.storageKey))
        let jsonText = String(decoding: rawJSON, as: UTF8.self)
        XCTAssertFalse(jsonText.contains("tok-123"))

        // Deleting token from Keychain makes load() return nil.
        keychain.delete(service: HostStore.keychainService, account: "a1b2c3d4e5f6")
        XCTAssertNil(store.load())
    }

    func testHostStoreRejectsUnpinnedNonLoopbackAndRespectsLoopbackFlag() throws {
        let suite = "PairingTests.validation.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = MemoryKeychain()

        // Non-loopback record without fingerprint → nil on load.
        let unpinned = try JSONEncoder().encode(
            HostStore.Record(host: "192.168.1.20", port: 38471, fingerprint: nil, deviceId: nil)
        )
        defaults.set(unpinned, forKey: HostStore.storageKey)
        XCTAssertNil(HostStore(defaults: defaults, keychain: keychain, allowLoopback: true).load())

        // Plaintext 127.0.0.1 record → allowed only when allowLoopback is true.
        let debugStore = HostStore(defaults: defaults, keychain: keychain, allowLoopback: true)
        let loopback = PairedHost(
            address: HostAddress(host: "127.0.0.1", port: 38471),
            fingerprint: nil,
            deviceId: nil,
            token: nil
        )
        try debugStore.save(loopback)
        XCTAssertEqual(debugStore.load(), loopback)

        let releaseStore = HostStore(defaults: defaults, keychain: keychain, allowLoopback: false)
        XCTAssertNil(releaseStore.load())
    }

    func testHostStoreKeychainFailurePreservesExistingHost() throws {
        let suite = "PairingTests.kcFail.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = MemoryKeychain()
        let store = HostStore(defaults: defaults, keychain: keychain, allowLoopback: false)

        let first = PairedHost(
            address: HostAddress(host: "10.0.0.1", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev-old",
            token: "tok-old"
        )
        try store.save(first)

        // Simulate Keychain write failure when saving a new Host.
        keychain.failNextWrite = KeychainError.unexpectedStatus(-25308)
        let second = PairedHost(
            address: HostAddress(host: "10.0.0.2", port: 38471),
            fingerprint: Self.otherFP,
            deviceId: "dev-new",
            token: "tok-new"
        )
        XCTAssertThrowsError(try store.save(second))

        // Existing record and Keychain item remain intact.
        XCTAssertEqual(store.load(), first)
        XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "dev-old"), "tok-old")
        XCTAssertNil(keychain.read(service: HostStore.keychainService, account: "dev-new"))

        // Succeeding save writes new item, updates defaults, and returns superseded deviceId without deleting yet.
        let superseded = try store.save(second)
        XCTAssertEqual(store.load(), second)
        XCTAssertEqual(superseded, "dev-old")
        XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "dev-old"), "tok-old")
        XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "dev-new"), "tok-new")
        if let superseded {
            store.deleteToken(forDeviceId: superseded)
        }
        XCTAssertNil(keychain.read(service: HostStore.keychainService, account: "dev-old"))

        // Updating address for the SAME deviceId keeps the Keychain item and reports no superseded id.
        var moved = second
        moved.address = HostAddress(host: "10.0.0.9", port: 38471)
        let sameSuperseded = try store.save(moved)
        XCTAssertNil(sameSuperseded)
        XCTAssertEqual(store.load(), moved)
        XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "dev-new"), "tok-new")
    }

    // MARK: HostError Copy & 401 Mapping

    func testHostErrorDescriptionsAnd401Mapping() {
        XCTAssertEqual(HostError.removed.errorDescription, "这台 iPhone 已被 Host 移除，请重新扫码配对。")
        XCTAssertEqual(HostError.certificateChanged.errorDescription, "Host 的证书与配对时不同，请重新扫码配对。")
        XCTAssertEqual(HostError.pairingExpired.errorDescription, "配对码已过期或已用过，请在 Host 上重新生成。")
        XCTAssertEqual(HostError.pairingUnreachable.errorDescription, "连不上二维码里的 Host。")
        XCTAssertEqual(HostError.unreachable("10.0.0.1:38471").errorDescription, "连不上 Host（10.0.0.1:38471）。")
        XCTAssertEqual(
            HostError.versionMismatch(host: 1).errorDescription,
            "Host 与 App 版本不一致（Host 接口版本 1，App 支持 2）。"
        )

        // HTTP 401 always maps to .removed regardless of body.
        for body in [Data(), Data("unauthorized".utf8), Data(#"{"error":{"code":"other","message":"nope"}}"#.utf8)] {
            XCTAssertThrowsError(try HostClient.decodeRPC(body, status: 401) as String) {
                XCTAssertEqual($0 as? HostError, .removed)
            }
        }
    }

    // MARK: HostClient HTTP Behavior

    func testHostClientAuthHeadersPairForwardAndGet() async throws {
        let host = PairedHost(
            address: HostAddress(host: "192.168.1.20", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev1",
            token: "my-secret-token"
        )
        let session = StubURLProtocol.makeSession()
        let client = HostClient(host: host, session: session)
        XCTAssertEqual(client.baseURL.absoluteString, "https://192.168.1.20:38471")

        let plainClient = HostClient(address: HostAddress(host: "127.0.0.1", port: 38471), session: session)
        XCTAssertEqual(plainClient.baseURL.absoluteString, "http://127.0.0.1:38471")

        // 1. /health must NOT send Authorization.
        StubURLProtocol.handler = { req in
            XCTAssertEqual(req.url?.absoluteString, "https://192.168.1.20:38471/health")
            XCTAssertNil(req.value(forHTTPHeaderField: "Authorization"))
            return (200, Data(#"{"product":"CoDeck","apiVersion":2}"#.utf8), [:])
        }
        let health = try await client.checkHealth(timeout: 3)
        XCTAssertEqual(health.apiVersion, 2)

        // 2. /v1/pair must NOT send Authorization; 200 returns (deviceId, token).
        StubURLProtocol.handler = { req in
            XCTAssertEqual(req.url?.absoluteString, "https://192.168.1.20:38471/v1/pair")
            XCTAssertNil(req.value(forHTTPHeaderField: "Authorization"))
            return (200, Data(#"{"result":{"deviceId":"aabbccddeeff","token":"new-tok"}}"#.utf8), [:])
        }
        let creds = try await client.pair(key: Self.validKey, name: "iPhone")
        XCTAssertEqual(creds.deviceId, "aabbccddeeff")
        XCTAssertEqual(creds.token, "new-tok")

        // 3. /v1/pair 403 → .pairingExpired.
        StubURLProtocol.handler = { _ in
            (403, Data(#"{"error":{"code":"pairing_expired","message":"expired"}}"#.utf8), [:])
        }
        do {
            _ = try await client.pair(key: Self.validKey, name: "iPhone")
            XCTFail("Expected pairingExpired")
        } catch let error as HostError {
            XCTAssertEqual(error, .pairingExpired)
        }

        // 4. forwardRPC sends Authorization and returns raw JSON body + status without decoding result.
        let rawRPC = Data(#"{"result":{"nested":[1,2,3]}}"#.utf8)
        StubURLProtocol.handler = { req in
            XCTAssertEqual(req.url?.absoluteString, "https://192.168.1.20:38471/v1/rpc")
            XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer my-secret-token")
            return (200, rawRPC, [:])
        }
        let (forwarded, forwardedResp) = try await client.forwardRPC(method: "get_session", args: .object(["sessionId": .string("s1")]))
        XCTAssertEqual(forwardedResp.statusCode, 200)
        XCTAssertEqual(forwarded, rawRPC)

        // 5. Authenticated GET preserves percent-encoded path + query (including empty query) and headers.
        StubURLProtocol.handler = { req in
            XCTAssertEqual(req.httpMethod, "GET")
            XCTAssertEqual(req.url?.absoluteString, "https://192.168.1.20:38471/web/a%20b/transcript.html?sessionId=abc%201&x=a%2Bb")
            XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer my-secret-token")
            return (200, Data("<html>ok</html>".utf8), ["Content-Type": "text/html"])
        }
        let (body, resp) = try await client.get(path: "/web/a%20b/transcript.html", query: "sessionId=abc%201&x=a%2Bb")
        XCTAssertEqual(resp.statusCode, 200)
        XCTAssertEqual(resp.value(forHTTPHeaderField: "Content-Type"), "text/html")
        XCTAssertEqual(String(decoding: body, as: UTF8.self), "<html>ok</html>")

        StubURLProtocol.handler = { req in
            XCTAssertEqual(req.url?.absoluteString, "https://192.168.1.20:38471/web/empty.js?")
            return (200, Data(), [:])
        }
        _ = try await client.get(path: "/web/empty.js", query: "")

        // 6. Bridge methods (`get` and `forwardRPC`) preserve raw 401 status and body; typed RPC maps 401 to `.removed`.
        let unauthorizedBody = Data(#"{"error":{"code":"unauthorized","message":"这台设备没有配对或已被移除，请重新扫码配对。"}}"#.utf8)
        StubURLProtocol.handler = { _ in (401, unauthorizedBody, ["Content-Type": "application/json"]) }
        let (get401Data, get401Resp) = try await client.get(path: "/web/transcript.html")
        XCTAssertEqual(get401Resp.statusCode, 401)
        XCTAssertEqual(get401Data, unauthorizedBody)

        let (rpc401Data, rpc401Resp) = try await client.forwardRPC(method: "get_session", args: .object([:]))
        XCTAssertEqual(rpc401Resp.statusCode, 401)
        XCTAssertEqual(rpc401Data, unauthorizedBody)

        do {
            try await client.checkAuth(timeout: 5)
            XCTFail("Expected removed for typed RPC 401")
        } catch let error as HostError {
            XCTAssertEqual(error, .removed)
        }
    }

    func testDecodePair403OnlyPairingExpiredMapsToPairingExpired() {
        // 403 with pairing_expired → .pairingExpired.
        let expiredBody = Data(#"{"error":{"code":"pairing_expired","message":"配对码已过期"}}"#.utf8)
        XCTAssertThrowsError(try HostClient.decodePair(expiredBody, status: 403)) {
            XCTAssertEqual($0 as? HostError, .pairingExpired)
        }

        // 403 with another error code (e.g. subnet/Host/Origin rejection) → .host preserving Host message.
        let subnetBody = Data(#"{"error":{"code":"forbidden","message":"不在允许的局域网内"}}"#.utf8)
        XCTAssertThrowsError(try HostClient.decodePair(subnetBody, status: 403)) {
            XCTAssertEqual($0 as? HostError, .host(code: "forbidden", message: "不在允许的局域网内"))
        }

        // 403 with JSON error message and no code → .host.
        let noCodeBody = Data(#"{"error":{"message":"Host 头不匹配"}}"#.utf8)
        XCTAssertThrowsError(try HostClient.decodePair(noCodeBody, status: 403)) {
            XCTAssertEqual($0 as? HostError, .host(code: nil, message: "Host 头不匹配"))
        }

        // Non-JSON 403 → .badResponse (must not misreport as pairingExpired).
        XCTAssertThrowsError(try HostClient.decodePair(Data("Forbidden".utf8), status: 403)) {
            XCTAssertEqual($0 as? HostError, .badResponse)
        }
    }

    func testHostStoreSameFingerprintReuseDoesNotRewriteKeychain() throws {
        let suite = "PairingTests.reuseNoRewrite.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = MemoryKeychain()
        let store = HostStore(defaults: defaults, keychain: keychain, allowLoopback: false)

        let initial = PairedHost(
            address: HostAddress(host: "10.0.0.1", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev-same",
            token: "tok-same"
        )
        try store.save(initial)
        XCTAssertEqual(keychain.writeCount, 1)

        // Arm a Keychain write failure: updating only the address must not call keychain.write at all.
        keychain.failNextWrite = KeychainError.unexpectedStatus(-25308)
        var moved = initial
        moved.address = HostAddress(host: "192.168.1.99", port: 38472)
        try store.save(moved)

        XCTAssertEqual(keychain.writeCount, 1)
        XCTAssertEqual(store.load(), moved)
        XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "dev-same"), "tok-same")
    }

    // MARK: Pairing Flow Steps

    func testPairingSameFingerprintValidTokenSkipsPairAndUpdatesAddress() async throws {
        let link = PairingLink(
            hosts: ["10.0.0.1", "10.0.0.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let existing = PairedHost(
            address: HostAddress(host: "192.168.1.5", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev-existing",
            token: "tok-existing"
        )
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { addr, fp, timeout in
                recorder.record("health:\(addr.host):\(Int(timeout))")
                XCTAssertEqual(fp, Self.validFP)
                if addr.host == "10.0.0.1" { throw HostError.unreachable(addr.display) }
                return HostHealth(product: "CoDeck", apiVersion: 2, version: nil)
            },
            checkToken: { addr, fp, token in
                recorder.record("checkToken:\(addr.host):\(token)")
                XCTAssertEqual(fp, Self.validFP)
            },
            pair: { _, _, _, _ in
                recorder.record("pair")
                return ("should-not-happen", "should-not-happen")
            },
            save: { host in
                recorder.record("save:\(host.host):\(host.deviceId ?? "")")
            }
        )

        let result = try await HostModel.executePairing(
            link: link,
            existing: existing,
            deviceName: "iPhone",
            dependencies: deps
        )
        XCTAssertEqual(result.address, HostAddress(host: "10.0.0.2", port: 38471))
        XCTAssertEqual(result.deviceId, "dev-existing")
        XCTAssertEqual(result.token, "tok-existing")
        XCTAssertEqual(recorder.calls, [
            "health:10.0.0.1:3",
            "health:10.0.0.2:3",
            "checkToken:10.0.0.2:tok-existing",
            "save:10.0.0.2:dev-existing",
        ])
    }

    func testPairingSameFingerprintToken401CallsPair() async throws {
        let link = PairingLink(
            hosts: ["10.0.0.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let existing = PairedHost(
            address: HostAddress(host: "10.0.0.2", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev-old",
            token: "tok-revoked"
        )
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { _, _, _ in HostHealth(product: "CoDeck", apiVersion: 2, version: nil) },
            checkToken: { _, _, _ in
                recorder.record("checkToken")
                throw HostError.removed
            },
            pair: { addr, _, key, name in
                recorder.record("pair:\(addr.host):\(key):\(name)")
                return ("dev-new", "tok-new")
            },
            save: { host in
                recorder.record("save:\(host.deviceId ?? "")")
            }
        )

        let result = try await HostModel.executePairing(
            link: link,
            existing: existing,
            deviceName: "My iPhone",
            dependencies: deps
        )
        XCTAssertEqual(result.deviceId, "dev-new")
        XCTAssertEqual(result.token, "tok-new")
        XCTAssertEqual(recorder.calls, [
            "checkToken",
            "pair:10.0.0.2:\(Self.validKey):My iPhone",
            "save:dev-new",
        ])
    }

    func testPairingSameFingerprintTokenNon401FailsWithoutCallingPair() async {
        let link = PairingLink(
            hosts: ["10.0.0.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let existing = PairedHost(
            address: HostAddress(host: "10.0.0.2", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev-old",
            token: "tok-old"
        )
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { _, _, _ in HostHealth(product: "CoDeck", apiVersion: 2, version: nil) },
            checkToken: { _, _, _ in
                recorder.record("checkToken")
                throw HostError.unreachable("10.0.0.2:38471")
            },
            pair: { _, _, _, _ in
                recorder.record("pair")
                return ("x", "y")
            },
            save: { _ in recorder.record("save") }
        )

        do {
            _ = try await HostModel.executePairing(
                link: link,
                existing: existing,
                deviceName: "iPhone",
                dependencies: deps
            )
            XCTFail("Expected pairingUnreachable")
        } catch let error as HostError {
            XCTAssertEqual(error, .pairingUnreachable)
            XCTAssertEqual(error.errorDescription, "连不上二维码里的 Host。")
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
        XCTAssertEqual(recorder.calls, ["checkToken"])
    }

    func testPairingAllAddressesFailHealthReportsPairingUnreachable() async {
        let link = PairingLink(
            hosts: ["10.0.0.1", "192.168.1.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { addr, _, _ in
                recorder.record("health:\(addr.host)")
                throw HostError.unreachable(addr.display)
            },
            checkToken: { _, _, _ in recorder.record("checkToken") },
            pair: { _, _, _, _ in
                recorder.record("pair")
                return ("x", "y")
            },
            save: { _ in recorder.record("save") }
        )

        do {
            _ = try await HostModel.executePairing(
                link: link,
                existing: nil,
                deviceName: "iPhone",
                dependencies: deps
            )
            XCTFail("Expected pairingUnreachable")
        } catch let error as HostError {
            XCTAssertEqual(error, .pairingUnreachable)
            XCTAssertEqual(error.errorDescription, "连不上二维码里的 Host。")
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
        XCTAssertEqual(recorder.calls, ["health:10.0.0.1", "health:192.168.1.2"])
    }

    // MARK: HostModel Terminal needsPairing, Generation & Serial Pairing

    @MainActor
    func testHostModelNeedsPairingIsTerminalUntilNextPair() async throws {
        let suite = "PairingTests.model.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = MemoryKeychain()
        let store = HostStore(defaults: defaults, keychain: keychain, allowLoopback: false)

        let initial = PairedHost(
            address: HostAddress(host: "10.0.0.1", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev1",
            token: "tok1"
        )
        try store.save(initial)

        let stubSession = StubURLProtocol.makeSession()
        StubURLProtocol.handler = { _ in (401, Data(), [:]) }

        let model = HostModel(
            store: store,
            clientFactory: { host, _ in HostClient(host: host, session: stubSession) },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )
        // Stop background WS loop immediately so the test drives disconnect attribution deterministically.
        model.pause()

        let gen1 = model.generation
        let client1 = try XCTUnwrap(model.client)
        model.handleDisconnect(attempt: 1, certMismatch: false, generation: gen1, client: client1)

        // Wait for async attribution task to complete.
        for _ in 0..<50 where !model.needsPairing {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertTrue(model.needsPairing)
        XCTAssertEqual(model.problem, HostError.removed.errorDescription)

        // Terminal: resume / retry / refresh / hello must NOT clear needsPairing or overwrite problem.
        StubURLProtocol.handler = { _ in (200, Data(#"{"product":"CoDeck","apiVersion":2}"#.utf8), [:]) }
        model.resume()
        model.retry()
        await model.refresh()
        model.handle(.hello(apiVersion: 2))
        XCTAssertTrue(model.needsPairing)
        XCTAssertEqual(model.problem, HostError.removed.errorDescription)

        // Pairing again clears needsPairing and increments generation.
        let link = PairingLink(
            hosts: ["10.0.0.2"],
            port: 38471,
            fingerprint: Self.otherFP,
            key: Self.validKey
        )
        let deps = PairingDependencies(
            checkHealth: { _, _, _ in HostHealth(product: "CoDeck", apiVersion: 2, version: nil) },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in ("dev2", "tok2") },
            save: { try store.save($0) }
        )
        let paired = try await model.pair(with: link, dependencies: deps)
        model.pause()
        XCTAssertTrue(paired)
        XCTAssertFalse(model.needsPairing)
        XCTAssertEqual(model.generation, gen1 + 1)
        XCTAssertEqual(model.host?.deviceId, "dev2")
    }

    @MainActor
    func testHostModelSerializesConcurrentPairCalls() async throws {
        let suite = "PairingTests.serial.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = HostStore(defaults: defaults, keychain: MemoryKeychain(), allowLoopback: false)
        let stubSession = StubURLProtocol.makeSession()
        StubURLProtocol.handler = { _ in (500, Data(), [:]) }

        let model = HostModel(
            store: store,
            clientFactory: { host, _ in HostClient(host: host, session: stubSession) },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )
        let link = PairingLink(
            hosts: ["10.0.0.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let gate = AsyncGate()
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { _, _, _ in
                recorder.record("health")
                await gate.wait()
                return HostHealth(product: "CoDeck", apiVersion: 2, version: nil)
            },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in
                recorder.record("pair")
                return ("dev1", "tok1")
            },
            save: { try store.save($0) }
        )

        let firstTask = Task { try await model.pair(with: link, dependencies: deps) }
        // Yield so the first pair call enters checkHealth and sets isPairing = true.
        for _ in 0..<50 where !model.isPairing {
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertTrue(model.isPairing)

        let secondResult = try await model.pair(with: link, dependencies: deps)
        XCTAssertFalse(secondResult)

        await gate.open()
        let firstResult = try await firstTask.value
        model.pause()
        XCTAssertTrue(firstResult)
        XCTAssertEqual(recorder.calls, ["health", "pair"])
    }

    // MARK: Regression — Same-Fingerprint Save Error, Resolve/Pair Split, Cancellation, Stale Start & Disconnect Probe

    func testPairingSameFingerprintSaveErrorPropagatesOriginalError() async {
        let link = PairingLink(
            hosts: ["10.0.0.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let existing = PairedHost(
            address: HostAddress(host: "10.0.0.1", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev-old",
            token: "tok-old"
        )
        let saveFailure = KeychainError.unexpectedStatus(-25308)
        let deps = PairingDependencies(
            checkHealth: { _, _, _ in HostHealth(product: "CoDeck", apiVersion: 2, version: nil) },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in
                XCTFail("pair should not be called when token is valid")
                return ("x", "y")
            },
            save: { _ in throw saveFailure }
        )

        do {
            _ = try await HostModel.executePairing(
                link: link,
                existing: existing,
                deviceName: "iPhone",
                dependencies: deps
            )
            XCTFail("Expected original KeychainError")
        } catch let error as KeychainError {
            XCTAssertEqual(error, saveFailure)
        } catch {
            XCTFail("Expected KeychainError, got \(error)")
        }
    }

    @MainActor
    func testHostModelResolveThenPairAtResolvedAddress() async throws {
        let suite = "PairingTests.resolveThenPair.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = HostStore(defaults: defaults, keychain: MemoryKeychain(), allowLoopback: false)
        let stubSession = StubURLProtocol.makeSession()
        StubURLProtocol.handler = { _ in (500, Data(), [:]) }

        let model = HostModel(
            store: store,
            clientFactory: { host, _ in HostClient(host: host, session: stubSession) },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )

        let link = PairingLink(
            hosts: ["10.0.0.1", "10.0.0.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { addr, fp, timeout in
                recorder.record("health:\(addr.host):\(Int(timeout))")
                XCTAssertEqual(fp, Self.validFP)
                if addr.host == "10.0.0.1" { throw HostError.unreachable(addr.display) }
                return HostHealth(product: "CoDeck", apiVersion: 2, version: nil)
            },
            checkToken: { _, _, _ in recorder.record("checkToken") },
            pair: { addr, _, _, _ in
                recorder.record("pair:\(addr.display)")
                return ("dev1", "tok1")
            },
            save: { host in
                recorder.record("save:\(host.display)")
                try store.save(host)
            }
        )

        // Step 1: resolve only probes /health and sends no credentials.
        let resolved = try await model.resolve(link, dependencies: deps)
        XCTAssertEqual(resolved, HostAddress(host: "10.0.0.2", port: 38471))
        XCTAssertEqual(recorder.calls, ["health:10.0.0.1:3", "health:10.0.0.2:3"])
        XCTAssertEqual(
            PairingPresentation.openURLConfirmMessage(for: resolved, hasCurrentHost: true),
            "与 Host 10.0.0.2:38471 配对？会替换当前的 Host。"
        )

        // Step 2: pair(with:at:) uses the resolved address directly without re-probing /health.
        let paired = try await model.pair(with: link, at: resolved, dependencies: deps)
        model.pause()
        XCTAssertTrue(paired)
        XCTAssertEqual(recorder.calls, [
            "health:10.0.0.1:3",
            "health:10.0.0.2:3",
            "pair:10.0.0.2:38471",
            "save:10.0.0.2:38471",
        ])
        XCTAssertEqual(model.address, resolved)
    }

    func testPairingCancellationStopsBeforePairAndBeforeSave() async throws {
        let link = PairingLink(
            hosts: ["10.0.0.1"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )

        // Case A: Cancelled during health probe → /v1/pair and save must NOT run.
        let gateA = AsyncGate()
        let recorderA = CallRecorder()
        let depsA = PairingDependencies(
            checkHealth: { _, _, _ in
                recorderA.record("health")
                await gateA.wait()
                return HostHealth(product: "CoDeck", apiVersion: 2, version: nil)
            },
            checkToken: { _, _, _ in recorderA.record("checkToken") },
            pair: { _, _, _, _ in
                recorderA.record("pair")
                return ("dev1", "tok1")
            },
            save: { _ in recorderA.record("save") }
        )
        let taskA = Task {
            try await HostModel.executePairing(link: link, existing: nil, deviceName: "iPhone", dependencies: depsA)
        }
        for _ in 0..<50 where recorderA.calls.isEmpty {
            try await Task.sleep(for: .milliseconds(5))
        }
        taskA.cancel()
        await gateA.open()
        do {
            _ = try await taskA.value
            XCTFail("Expected CancellationError")
        } catch is CancellationError {}
        XCTAssertEqual(recorderA.calls, ["health"])

        // Case B: Cancelled while /v1/pair is in flight → save must NOT run.
        let gateB = AsyncGate()
        let recorderB = CallRecorder()
        let depsB = PairingDependencies(
            checkHealth: { _, _, _ in HostHealth(product: "CoDeck", apiVersion: 2, version: nil) },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in
                recorderB.record("pair")
                await gateB.wait()
                return ("dev2", "tok2")
            },
            save: { _ in recorderB.record("save") }
        )
        let taskB = Task {
            try await HostModel.executePairing(
                link: link,
                at: HostAddress(host: "10.0.0.1", port: 38471),
                existing: nil,
                deviceName: "iPhone",
                dependencies: depsB
            )
        }
        for _ in 0..<50 where recorderB.calls.isEmpty {
            try await Task.sleep(for: .milliseconds(5))
        }
        taskB.cancel()
        await gateB.open()
        do {
            _ = try await taskB.value
            XCTFail("Expected CancellationError")
        } catch is CancellationError {}
        XCTAssertEqual(recorderB.calls, ["pair"])
    }

    @MainActor
    func testHostModelValidHelloInvalidatesInFlightDisconnectProbe() async throws {
        let suite = "PairingTests.helloInvalidatesProbe.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = HostStore(defaults: defaults, keychain: MemoryKeychain(), allowLoopback: false)

        let initial = PairedHost(
            address: HostAddress(host: "10.0.0.1", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev1",
            token: "tok1"
        )
        try store.save(initial)

        let probeEntered = CallRecorder()
        let releaseProbe = DispatchSemaphore(value: 0)
        let stubSession = StubURLProtocol.makeSession()
        StubURLProtocol.handler = { req in
            if req.url?.path == "/v1/rpc",
               String(decoding: StubURLProtocol.body(of: req), as: UTF8.self).contains("get_app_info") {
                probeEntered.record("probe")
                _ = releaseProbe.wait(timeout: .now() + 5)
                // Even a 200 (or 401) returning after valid `hello` must be discarded.
                return (200, Data(#"{"result":{}}"#.utf8), [:])
            }
            if req.url?.path == "/health" {
                return (200, Data(#"{"product":"CoDeck","apiVersion":2}"#.utf8), [:])
            }
            return (200, Data(#"{"result":[]}"#.utf8), [:])
        }

        let model = HostModel(
            store: store,
            clientFactory: { host, _ in HostClient(host: host, session: stubSession) },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )
        model.pause()
        let gen = model.generation
        let client = try XCTUnwrap(model.client)

        model.handleDisconnect(attempt: 1, certMismatch: false, generation: gen, client: client)
        for _ in 0..<200 where probeEntered.calls.isEmpty {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertEqual(probeEntered.calls, ["probe"])

        // Valid hello arrives before the slow disconnect probe finishes.
        model.handle(.hello(apiVersion: 2))
        releaseProbe.signal()
        try await Task.sleep(for: .milliseconds(50))

        XCTAssertNil(model.problem)
        XCTAssertFalse(model.needsPairing)
    }

    @MainActor
    func testConnectStoreStartResumeForkDropStaleResultsAfterReset() async throws {
        let store = ConnectStore()
        let hostA = PairedHost(address: HostAddress(host: "10.0.0.1", port: 38471), fingerprint: Self.validFP, deviceId: "dA", token: "tA")
        let hostB = PairedHost(address: HostAddress(host: "10.0.0.2", port: 38471), fingerprint: Self.otherFP, deviceId: "dB", token: "tB")

        let entered = CallRecorder()
        let reqRelease = DispatchSemaphore(value: 0)
        let session = StubURLProtocol.makeSession()
        let clientA = HostClient(host: hostA, session: session)
        let clientB = HostClient(host: hostB, session: session)

        // 1. startNew in-flight across reset → throws CancellationError and does not write to new Host.
        StubURLProtocol.handler = { _ in
            entered.record("start")
            _ = reqRelease.wait(timeout: .now() + 5)
            return (200, Data(#"{"result":"conn-old"}"#.utf8), [:])
        }
        store.reset(client: clientA)
        let startTask = Task { try await store.startNew(cwd: "/tmp/a") }
        for _ in 0..<200 where entered.calls.count < 1 {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertEqual(entered.calls.count, 1)
        store.reset(client: clientB)
        reqRelease.signal()
        do {
            _ = try await startTask.value
            XCTFail("Expected CancellationError when Host switched during startNew")
        } catch is CancellationError {}
        XCTAssertTrue(store.starts.isEmpty)
        XCTAssertTrue(store.errors.isEmpty)

        // 2. resume failure in-flight across reset → does not write errors or clear new Host's pendingStarts.
        StubURLProtocol.handler = { _ in
            entered.record("resume")
            _ = reqRelease.wait(timeout: .now() + 5)
            return (200, Data(#"{"error":{"code":"boom","message":"old failure"}}"#.utf8), [:])
        }
        store.reset(client: clientA)
        let resumeTask = Task { await store.resume(sessionId: "s1", cwd: "/tmp/a") }
        for _ in 0..<200 where entered.calls.count < 2 {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertEqual(entered.calls.count, 2)
        store.reset(client: clientB)
        let newKind = StartKind.resume(sessionId: "s1")
        store.pendingStarts.insert(newKind)
        reqRelease.signal()
        await resumeTask.value
        XCTAssertTrue(store.starts.isEmpty)
        XCTAssertNil(store.errors["s1"])
        // Old defer must NOT remove new Host's pendingStarts entry.
        XCTAssertTrue(store.pendingStarts.contains(newKind))

        // 3. fork success in-flight across reset → does not write starts or remove new Host's pendingStarts.
        StubURLProtocol.handler = { _ in
            entered.record("fork")
            _ = reqRelease.wait(timeout: .now() + 5)
            return (200, Data(#"{"result":"conn-fork-old"}"#.utf8), [:])
        }
        store.reset(client: clientA)
        let forkTask = Task { await store.fork(sessionId: "s1", cwd: "/tmp/a", title: "Fork") }
        for _ in 0..<200 where entered.calls.count < 3 {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertEqual(entered.calls.count, 3)
        store.reset(client: clientB)
        let newForkKind = StartKind.fork(origin: "s1")
        store.pendingStarts.insert(newForkKind)
        reqRelease.signal()
        await forkTask.value
        XCTAssertTrue(store.starts.isEmpty)
        XCTAssertNil(store.errors["s1"])
        XCTAssertTrue(store.pendingStarts.contains(newForkKind))
    }

    // MARK: Presentation Helpers (URL Classification, URL Confirm, Setup Notice & OfflineCard)

    func testClassifyOpenURLHandlesMalformedPairAndIgnoresOtherLinks() throws {
        let validURL = URL(string: "codeck://pair?h=192.168.1.20&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)")!
        if case .pairingLink(let link) = PairingPresentation.classifyOpenURL(validURL) {
            XCTAssertEqual(link.firstHost, "192.168.1.20")
        } else {
            XCTFail("Expected .pairingLink")
        }

        for bad in [
            "codeck://pair",
            "codeck://pair?h=192.168.1.20&p=38471&fp=\(Self.validFP)",
            "codeck://pair/extra?h=192.168.1.20&p=38471&fp=\(Self.validFP)&k=\(Self.validKey)",
        ] {
            XCTAssertEqual(
                PairingPresentation.classifyOpenURL(URL(string: bad)!),
                .invalidPairingLink("这不是 CoDeck 的配对二维码。"),
                bad
            )
        }

        for ignored in [
            "codeck://open?sessionId=s1",
            "codeck://connect",
            "https://example.com/pair",
        ] {
            XCTAssertEqual(
                PairingPresentation.classifyOpenURL(URL(string: ignored)!),
                .ignore,
                ignored
            )
        }
    }

    func testOpenURLConfirmMessageAndSetupNotice() {
        let address = HostAddress(host: "192.168.1.20", port: 38471)
        XCTAssertEqual(
            PairingPresentation.openURLConfirmMessage(for: address, hasCurrentHost: false),
            "与 Host 192.168.1.20:38471 配对？"
        )
        XCTAssertEqual(
            PairingPresentation.openURLConfirmMessage(for: address, hasCurrentHost: true),
            "与 Host 192.168.1.20:38471 配对？会替换当前的 Host。"
        )
        XCTAssertNil(PairingPresentation.setupReplacementNotice(hasCurrentHost: false))
        XCTAssertEqual(
            PairingPresentation.setupReplacementNotice(hasCurrentHost: true),
            "扫码后会替换当前的 Host。"
        )
    }

    func testOfflineCardShowsRetryAndReturnToListRules() {
        XCTAssertTrue(PairingPresentation.showsRetryInOfflineCard(needsPairing: false))
        XCTAssertFalse(PairingPresentation.showsRetryInOfflineCard(needsPairing: true))
        XCTAssertFalse(PairingPresentation.shouldReturnToSessionList(needsPairing: false))
        XCTAssertTrue(PairingPresentation.shouldReturnToSessionList(needsPairing: true))
    }

    // MARK: Regression Round 2 — Resolve Timeout/Cancel, 16 KB Streaming Cap, Redeem Phase, Keychain Order & Rewind Guard

    func testResolveEnforcesTotalTimeoutAndCancelsInFlightProbe() async {
        let link = PairingLink(
            hosts: ["10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { addr, _, _ in
                recorder.record("start:\(addr.host)")
                do {
                    try await Task.sleep(for: .seconds(5))
                } catch is CancellationError {
                    recorder.record("cancelled:\(addr.host)")
                    throw CancellationError()
                }
                return HostHealth(product: "CoDeck", apiVersion: 2, version: nil)
            },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in ("x", "y") },
            save: { _ in }
        )

        let started = ContinuousClock.now
        do {
            _ = try await HostModel.resolveAddress(
                for: link,
                dependencies: deps,
                totalTimeout: .milliseconds(150)
            )
            XCTFail("Expected pairingUnreachable on total timeout")
        } catch let error as HostError {
            XCTAssertEqual(error, .pairingUnreachable)
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
        let elapsed = started.duration(to: .now)
        XCTAssertLessThan(elapsed, .seconds(2))
        XCTAssertEqual(recorder.calls, ["start:10.0.0.1", "cancelled:10.0.0.1"])
    }

    func testResolveCancellationStopsProbingWithoutTryingNextCandidate() async throws {
        let link = PairingLink(
            hosts: ["10.0.0.1", "10.0.0.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { addr, _, _ in
                recorder.record("start:\(addr.host)")
                do {
                    try await Task.sleep(for: .seconds(5))
                } catch is CancellationError {
                    recorder.record("cancelled:\(addr.host)")
                    throw CancellationError()
                }
                return HostHealth(product: "CoDeck", apiVersion: 2, version: nil)
            },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in ("x", "y") },
            save: { _ in }
        )

        let task = Task {
            try await HostModel.resolveAddress(for: link, dependencies: deps, totalTimeout: .seconds(10))
        }
        for _ in 0..<100 where recorder.calls.isEmpty {
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertEqual(recorder.calls, ["start:10.0.0.1"])

        task.cancel()
        do {
            _ = try await task.value
            XCTFail("Expected CancellationError")
        } catch is CancellationError {}
        XCTAssertEqual(recorder.calls, ["start:10.0.0.1", "cancelled:10.0.0.1"])
    }

    func testHealthProbeAbortsOversizedStreamingResponseMidStream() async throws {
        let address = HostAddress(host: "10.0.0.1", port: 38471)
        let pinned = PinnedSession(expectedFingerprint: nil, protocolClasses: [StreamingHealthURLProtocol.self])
        let client = HostClient(address: address, pinnedSession: pinned)
        defer { StreamingHealthURLProtocol.config = nil }

        // 1. Chunked stream (unknown Content-Length) of 64 * 4 KiB = 256 KiB aborts mid-stream once > 16 KiB.
        let chunkedTracker = StreamingHealthTracker()
        StreamingHealthURLProtocol.config = .init(
            statusCode: 200,
            declaredLength: nil,
            chunkSize: 4096,
            totalChunks: 64,
            chunkIntervalMs: 5,
            tracker: chunkedTracker
        )
        do {
            _ = try await client.checkHealth(timeout: 5)
            XCTFail("Expected badResponse for oversized chunked /health")
        } catch let error as HostError {
            XCTAssertEqual(error, .badResponse)
        }
        for _ in 0..<50 where !chunkedTracker.stopped {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertTrue(chunkedTracker.stopped)
        XCTAssertLessThan(chunkedTracker.chunksSent, 20, "Must abort streaming long before sending all 64 chunks")

        // 2. Declared Content-Length > 16 KiB aborts on response headers before any body chunk is read.
        let headerTracker = StreamingHealthTracker()
        StreamingHealthURLProtocol.config = .init(
            statusCode: 200,
            declaredLength: HostClient.healthMaxBytes + 1,
            chunkSize: 4096,
            totalChunks: 10,
            chunkIntervalMs: 5,
            tracker: headerTracker
        )
        do {
            _ = try await client.checkHealth(timeout: 5)
            XCTFail("Expected badResponse for oversized Content-Length /health")
        } catch let error as HostError {
            XCTAssertEqual(error, .badResponse)
        }
        for _ in 0..<50 where !headerTracker.stopped {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertTrue(headerTracker.stopped)
        XCTAssertEqual(headerTracker.chunksSent, 0)

        // 3. Exact 16 KiB valid JSON succeeds; 16 KiB + 1 byte fails (also covers injected URLSession fallback).
        let baseJSON = #"{"product":"CoDeck","apiVersion":2}"#
        let exact16K = Data((baseJSON + String(repeating: " ", count: HostClient.healthMaxBytes - baseJSON.utf8.count)).utf8)
        XCTAssertEqual(exact16K.count, HostClient.healthMaxBytes)

        let stubSession = StubURLProtocol.makeSession()
        let fallbackClient = HostClient(address: address, session: stubSession)
        StubURLProtocol.handler = { _ in (200, exact16K, ["Content-Type": "application/json"]) }
        let okHealth = try await fallbackClient.checkHealth(timeout: 5)
        XCTAssertEqual(okHealth.apiVersion, 2)

        var over16K = exact16K
        over16K.append(UInt8(ascii: " "))
        StubURLProtocol.handler = { _ in (200, over16K, ["Content-Type": "application/json"]) }
        do {
            _ = try await fallbackClient.checkHealth(timeout: 5)
            XCTFail("Expected badResponse for 16 KiB + 1 byte /health")
        } catch let error as HostError {
            XCTAssertEqual(error, .badResponse)
        }
    }

    @MainActor
    func testHostModelIsRedeemingOnlyDuringPairExchange() async throws {
        let suite = "PairingTests.redeemPhase.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = HostStore(defaults: defaults, keychain: MemoryKeychain(), allowLoopback: false)
        let stubSession = StubURLProtocol.makeSession()
        StubURLProtocol.handler = { _ in (500, Data(), [:]) }

        let model = HostModel(
            store: store,
            clientFactory: { host, _ in HostClient(host: host, session: stubSession) },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )

        let link = PairingLink(
            hosts: ["10.0.0.2"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        let healthGate = AsyncGate()
        let pairGate = AsyncGate()
        let recorder = CallRecorder()
        let deps = PairingDependencies(
            checkHealth: { _, _, _ in
                recorder.record("health")
                await healthGate.wait()
                return HostHealth(product: "CoDeck", apiVersion: 2, version: nil)
            },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in
                recorder.record("pair")
                await pairGate.wait()
                return ("dev1", "tok1")
            },
            save: { try store.save($0) }
        )

        let task = Task { try await model.pair(with: link, dependencies: deps) }
        for _ in 0..<100 where !recorder.calls.contains("health") {
            try await Task.sleep(for: .milliseconds(5))
        }
        // Probing phase: pairing is active, redemption has NOT started yet (dismiss/cancel still allowed).
        XCTAssertTrue(model.isPairing)
        XCTAssertFalse(model.isRedeeming)

        await healthGate.open()
        for _ in 0..<100 where !recorder.calls.contains("pair") {
            try await Task.sleep(for: .milliseconds(5))
        }
        // Redemption phase: `/v1/pair` is in flight (sheet dismissal locked).
        XCTAssertTrue(model.isPairing)
        XCTAssertTrue(model.isRedeeming)

        await pairGate.open()
        _ = try await task.value
        model.pause()
        XCTAssertFalse(model.isPairing)
        XCTAssertFalse(model.isRedeeming)
    }

    @MainActor
    func testHostModelDeletesOldKeychainItemOnlyAfterUseSwitchesConnection() async throws {
        let suite = "PairingTests.keychainOrder.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let recorder = CallRecorder()
        let keychain = MemoryKeychain()
        let store = HostStore(defaults: defaults, keychain: keychain, allowLoopback: false)

        let oldHost = PairedHost(
            address: HostAddress(host: "10.0.0.1", port: 38471),
            fingerprint: Self.validFP,
            deviceId: "dev-old",
            token: "tok-old"
        )
        try store.save(oldHost)

        let stubSession = StubURLProtocol.makeSession()
        StubURLProtocol.handler = { _ in (500, Data(), [:]) }

        let model = HostModel(
            store: store,
            clientFactory: { host, _ in
                recorder.record("use:\(host.deviceId ?? "")")
                if host.deviceId == "dev-new" {
                    // During `use(paired)`, the old Keychain item must still exist (deleted only after switch).
                    XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "dev-old"), "tok-old")
                }
                return HostClient(host: host, session: stubSession)
            },
            streamFactory: { host, _ in EventStream(host: host, session: stubSession) }
        )
        model.pause()
        // Start recording Keychain mutations after initial load.
        keychain.onWrite = { account in recorder.record("kc:write:\(account)") }
        keychain.onDelete = { account in recorder.record("kc:delete:\(account)") }

        let link = PairingLink(
            hosts: ["10.0.0.2"],
            port: 38471,
            fingerprint: Self.otherFP,
            key: Self.validKey
        )
        let deps = PairingDependencies(
            checkHealth: { _, _, _ in HostHealth(product: "CoDeck", apiVersion: 2, version: nil) },
            checkToken: { _, _, _ in },
            pair: { _, _, _, _ in ("dev-new", "tok-new") },
            save: { try store.save($0) }
        )

        let paired = try await model.pair(with: link, dependencies: deps)
        model.pause()
        XCTAssertTrue(paired)
        XCTAssertNil(keychain.read(service: HostStore.keychainService, account: "dev-old"))
        XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "dev-new"), "tok-new")
        XCTAssertEqual(recorder.calls, [
            "use:dev-old",
            "kc:write:dev-new",
            "use:dev-new",
            "kc:delete:dev-old",
        ])
    }

    @MainActor
    func testConnectStoreRewindAndAnswerRewindDropStaleWritesAfterReset() async throws {
        let store = ConnectStore()
        let hostA = PairedHost(address: HostAddress(host: "10.0.0.1", port: 38471), fingerprint: Self.validFP, deviceId: "dA", token: "tA")
        let hostB = PairedHost(address: HostAddress(host: "10.0.0.2", port: 38471), fingerprint: Self.otherFP, deviceId: "dB", token: "tB")

        let entered = CallRecorder()
        let reqRelease = DispatchSemaphore(value: 0)
        let session = StubURLProtocol.makeSession()
        let clientA = HostClient(host: hostA, session: session)
        let clientB = HostClient(host: hostB, session: session)

        var conn = ConnState(conn: "c1")
        conn.sessionId = "s1"
        conn.ready = true
        conn.state = .idle

        // 1. rewind failure in-flight across reset → must not write errors or clear new Host's rewinding state.
        StubURLProtocol.handler = { _ in
            entered.record("rewind")
            _ = reqRelease.wait(timeout: .now() + 5)
            return (200, Data(#"{"result":{"success":false,"error":"old rewind err"}}"#.utf8), [:])
        }
        store.reset(client: clientA)
        let rewindTask = Task { await store.rewind(conn, uuid: "u-old") }
        for _ in 0..<200 where entered.calls.count < 1 {
            try await Task.sleep(for: .milliseconds(10))
        }
        store.reset(client: clientB)
        store.rewinding["c1"] = "u-new"
        reqRelease.signal()
        await rewindTask.value
        XCTAssertNil(store.errors["s1"])
        XCTAssertEqual(store.rewinding["c1"], "u-new")

        // 2. answerRewind success (.both with restored text) across reset → must not write draft or rewounds.
        StubURLProtocol.handler = { _ in
            entered.record("answer-both")
            _ = reqRelease.wait(timeout: .now() + 5)
            return (200, Data(#"{"result":{"success":true,"data":{"text":"stale draft"}}}"#.utf8), [:])
        }
        let req = RewindRequest(requestId: "r1", uuid: "u-old", options: [.both, .conversation, .code])
        store.reset(client: clientA)
        let answerBothTask = Task { await store.answerRewind(conn, req, .both) }
        for _ in 0..<200 where entered.calls.count < 2 {
            try await Task.sleep(for: .milliseconds(10))
        }
        store.reset(client: clientB)
        reqRelease.signal()
        let bothErr = await answerBothTask.value
        XCTAssertNil(bothErr)
        XCTAssertNil(store.restoredPrompts["c1"])
        XCTAssertNil(store.rewounds["c1"])
        XCTAssertNil(store.notes["c1"])

        // 3. answerRewind failure across reset → returns nil (no stale error shown).
        StubURLProtocol.handler = { _ in
            entered.record("answer-fail")
            _ = reqRelease.wait(timeout: .now() + 5)
            return (200, Data(#"{"result":{"success":false,"error":"old answer err"}}"#.utf8), [:])
        }
        store.reset(client: clientA)
        let answerFailTask = Task { await store.answerRewind(conn, req, .conversation) }
        for _ in 0..<200 where entered.calls.count < 3 {
            try await Task.sleep(for: .milliseconds(10))
        }
        store.reset(client: clientB)
        reqRelease.signal()
        let failErr = await answerFailTask.value
        XCTAssertNil(failErr)

        // 4. answerRewind (.code) across reset → must not write notes.
        StubURLProtocol.handler = { _ in
            entered.record("answer-code")
            _ = reqRelease.wait(timeout: .now() + 5)
            return (200, Data(#"{"result":{"success":true}}"#.utf8), [:])
        }
        store.reset(client: clientA)
        let answerCodeTask = Task { await store.answerRewind(conn, req, .code) }
        for _ in 0..<200 where entered.calls.count < 4 {
            try await Task.sleep(for: .milliseconds(10))
        }
        store.reset(client: clientB)
        reqRelease.signal()
        let codeErr = await answerCodeTask.value
        XCTAssertNil(codeErr)
        XCTAssertNil(store.notes["c1"])
    }
}

// MARK: - Test Doubles

private final class MemoryKeychain: KeychainStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [String: String] = [:]
    private var writes = 0
    var failNextWrite: Error?
    var onWrite: (@Sendable (String) -> Void)?
    var onDelete: (@Sendable (String) -> Void)?

    var writeCount: Int { lock.withLock { writes } }

    func read(service: String, account: String) -> String? {
        lock.withLock { storage["\(service):\(account)"] }
    }

    func write(_ value: String, service: String, account: String) throws {
        let cb: (@Sendable (String) -> Void)? = try lock.withLock {
            if let error = failNextWrite {
                failNextWrite = nil
                throw error
            }
            writes += 1
            storage["\(service):\(account)"] = value
            return onWrite
        }
        cb?(account)
    }

    func delete(service: String, account: String) {
        let cb: (@Sendable (String) -> Void)? = lock.withLock {
            _ = storage.removeValue(forKey: "\(service):\(account)")
            return onDelete
        }
        cb?(account)
    }
}

private final class CallRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [String] = []

    var calls: [String] { lock.withLock { items } }
    func record(_ entry: String) { lock.withLock { items.append(entry) } }
}

private actor AsyncGate {
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

private final class StubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: ((URLRequest) throws -> (Int, Data, [String: String]))?

    static func makeSession() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubURLProtocol.self]
        return URLSession(configuration: config)
    }

    static func body(of request: URLRequest) -> Data {
        if let data = request.httpBody { return data }
        guard let stream = request.httpBodyStream else { return Data() }
        stream.open()
        defer { stream.close() }
        var out = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count <= 0 { break }
            out.append(buffer, count: count)
        }
        return out
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

private final class StreamingHealthTracker: @unchecked Sendable {
    private let lock = NSLock()
    private var _chunksSent = 0
    private var _stopped = false

    var chunksSent: Int { lock.withLock { _chunksSent } }
    var stopped: Bool { lock.withLock { _stopped } }

    func incrementChunks() { lock.withLock { _chunksSent += 1 } }
    func markStopped() { lock.withLock { _stopped = true } }
}

private final class StreamingHealthURLProtocol: URLProtocol {
    struct Config {
        var statusCode: Int
        var declaredLength: Int?
        var chunkSize: Int
        var totalChunks: Int
        var chunkIntervalMs: Int
        var tracker: StreamingHealthTracker
    }

    nonisolated(unsafe) static var config: Config?

    private let stopLock = NSLock()
    private var stopped = false
    private var activeTracker: StreamingHealthTracker?

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let cfg = Self.config, let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.unknown))
            return
        }
        stopLock.withLock { activeTracker = cfg.tracker }
        var headers: [String: String] = ["Content-Type": "application/json"]
        if let declaredLength = cfg.declaredLength {
            headers["Content-Length"] = "\(declaredLength)"
        }
        let response = HTTPURLResponse(
            url: url,
            statusCode: cfg.statusCode,
            httpVersion: "HTTP/1.1",
            headerFields: headers
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)

        let chunk = Data(repeating: UInt8(ascii: "x"), count: cfg.chunkSize)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            for _ in 0..<cfg.totalChunks {
                Thread.sleep(forTimeInterval: Double(cfg.chunkIntervalMs) / 1000.0)
                if self.isStopped { return }
                cfg.tracker.incrementChunks()
                self.client?.urlProtocol(self, didLoad: chunk)
            }
            if !self.isStopped {
                self.client?.urlProtocolDidFinishLoading(self)
            }
        }
    }

    override func stopLoading() {
        let tracker: StreamingHealthTracker? = stopLock.withLock {
            stopped = true
            return activeTracker
        }
        tracker?.markStopped()
    }

    private var isStopped: Bool {
        stopLock.withLock { stopped }
    }
}
