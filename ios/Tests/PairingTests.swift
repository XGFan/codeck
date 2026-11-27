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

        // Succeeding save writes new item, updates defaults, and deletes old Keychain item.
        try store.save(second)
        XCTAssertEqual(store.load(), second)
        XCTAssertNil(keychain.read(service: HostStore.keychainService, account: "dev-old"))
        XCTAssertEqual(keychain.read(service: HostStore.keychainService, account: "dev-new"), "tok-new")

        // Updating address for the SAME deviceId keeps the Keychain item.
        var moved = second
        moved.address = HostAddress(host: "10.0.0.9", port: 38471)
        try store.save(moved)
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

        // 4. forwardRPC sends Authorization and returns raw JSON body without decoding result.
        let rawRPC = Data(#"{"result":{"nested":[1,2,3]}}"#.utf8)
        StubURLProtocol.handler = { req in
            XCTAssertEqual(req.url?.absoluteString, "https://192.168.1.20:38471/v1/rpc")
            XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer my-secret-token")
            return (200, rawRPC, [:])
        }
        let forwarded = try await client.forwardRPC(method: "get_session", args: .object(["sessionId": .string("s1")]))
        XCTAssertEqual(forwarded, rawRPC)

        // 5. Authenticated GET preserves path + query and headers.
        StubURLProtocol.handler = { req in
            XCTAssertEqual(req.httpMethod, "GET")
            XCTAssertEqual(req.url?.absoluteString, "https://192.168.1.20:38471/web/transcript.html?sessionId=abc%201")
            XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer my-secret-token")
            return (200, Data("<html>ok</html>".utf8), ["Content-Type": "text/html"])
        }
        let (body, resp) = try await client.get(path: "/web/transcript.html", query: "sessionId=abc%201")
        XCTAssertEqual(resp.statusCode, 200)
        XCTAssertEqual(resp.value(forHTTPHeaderField: "Content-Type"), "text/html")
        XCTAssertEqual(String(decoding: body, as: UTF8.self), "<html>ok</html>")

        // 6. Any HTTP 401 maps to .removed regardless of body.
        StubURLProtocol.handler = { _ in (401, Data("nope".utf8), [:]) }
        do {
            _ = try await client.get(path: "/web/transcript.html")
            XCTFail("Expected removed")
        } catch let error as HostError {
            XCTAssertEqual(error, .removed)
        }
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

    // MARK: Presentation Helpers (URL Confirm & OfflineCard)

    func testOpenURLConfirmMessage() {
        let link = PairingLink(
            hosts: ["192.168.1.20", "10.0.0.5"],
            port: 38471,
            fingerprint: Self.validFP,
            key: Self.validKey
        )
        XCTAssertEqual(
            PairingPresentation.openURLConfirmMessage(for: link, hasCurrentHost: false),
            "与 Host 192.168.1.20 配对？"
        )
        XCTAssertEqual(
            PairingPresentation.openURLConfirmMessage(for: link, hasCurrentHost: true),
            "与 Host 192.168.1.20 配对？会替换当前的 Host。"
        )
    }

    func testOfflineCardShowsRetryRule() {
        XCTAssertTrue(PairingPresentation.showsRetryInOfflineCard(needsPairing: false))
        XCTAssertFalse(PairingPresentation.showsRetryInOfflineCard(needsPairing: true))
    }
}

// MARK: - Test Doubles

private final class MemoryKeychain: KeychainStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [String: String] = [:]
    var failNextWrite: Error?

    func read(service: String, account: String) -> String? {
        lock.withLock { storage["\(service):\(account)"] }
    }

    func write(_ value: String, service: String, account: String) throws {
        try lock.withLock {
            if let error = failNextWrite {
                failNextWrite = nil
                throw error
            }
            storage["\(service):\(account)"] = value
        }
    }

    func delete(service: String, account: String) {
        lock.withLock { _ = storage.removeValue(forKey: "\(service):\(account)") }
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
