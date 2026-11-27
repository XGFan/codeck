import Foundation
import XCTest
@testable import CoDeck

/// Live TLS + WSS + auth probe against a real Host.
/// Runs only when `CODECK_PROBE=<IP>:<port>:<fingerprint>[:<token>]` is present in the test runner
/// environment (pass via `TEST_RUNNER_CODECK_PROBE=...` to `xcodebuild test`).
final class HostProbeTests: XCTestCase {
    private struct ProbeConfig {
        var address: HostAddress
        var fingerprint: String
        var token: String?
    }

    private func loadProbe() throws -> ProbeConfig {
        guard let raw = ProcessInfo.processInfo.environment["CODECK_PROBE"],
              !raw.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            throw XCTSkip("Set TEST_RUNNER_CODECK_PROBE=<IP>:<port>:<fingerprint>[:<token>] to run real Host probe tests.")
        }
        let parts = raw.split(separator: ":", maxSplits: 3, omittingEmptySubsequences: false).map(String.init)
        guard parts.count == 3 || parts.count == 4, let port = Int(parts[1]), (1...65535).contains(port) else {
            XCTFail("Invalid CODECK_PROBE format (expected <IP>:<port>:<fingerprint>[:<token>]): \(raw)")
            throw XCTSkip("Invalid CODECK_PROBE format")
        }
        let token: String? = parts.count == 4 && !parts[3].isEmpty ? parts[3] : nil
        return ProbeConfig(
            address: HostAddress(host: parts[0], port: port),
            fingerprint: parts[2],
            token: token
        )
    }

    private func corruptedFingerprint(_ fp: String) -> String {
        var chars = Array(fp)
        chars[0] = chars[0] == "A" ? "B" : "A"
        return String(chars)
    }

    func testHealthWithCorrectFingerprint() async throws {
        let probe = try loadProbe()
        let client = HostClient(address: probe.address, fingerprint: probe.fingerprint)
        let health = try await client.checkHealth(timeout: 5)
        XCTAssertEqual(health.product, "CoDeck")
        XCTAssertEqual(health.apiVersion, HostClient.apiVersion)
    }

    @MainActor
    func testWebSocketHelloWithCorrectFingerprint() async throws {
        let probe = try loadProbe()
        let host = PairedHost(
            address: probe.address,
            fingerprint: probe.fingerprint,
            deviceId: nil,
            token: probe.token
        )
        let stream = EventStream(host: host)
        defer { stream.stop() }

        let helloExpectation = expectation(description: "Receive hello event over WSS")
        var receivedVersion: Int?
        stream.onEvent = { event in
            if case .hello(let apiVersion) = event, receivedVersion == nil {
                receivedVersion = apiVersion
                helloExpectation.fulfill()
            }
        }
        stream.start()

        await fulfillment(of: [helloExpectation], timeout: 5)
        XCTAssertEqual(receivedVersion, HostClient.apiVersion)
    }

    func testWrongFingerprintReportsCertificateChanged() async throws {
        let probe = try loadProbe()
        let wrongFP = corruptedFingerprint(probe.fingerprint)

        // HTTP checkHealth with wrong fingerprint must be classified as .certificateChanged.
        let client = HostClient(address: probe.address, fingerprint: wrongFP)
        do {
            _ = try await client.checkHealth(timeout: 5)
            XCTFail("Expected checkHealth to fail with wrong fingerprint")
        } catch let error as HostError {
            XCTAssertEqual(error, .certificateChanged)
        }

        // WebSocket task via PinnedSession must also record a per-task fingerprint mismatch.
        let pinned = PinnedSession(expectedFingerprint: wrongFP)
        let host = PairedHost(
            address: probe.address,
            fingerprint: wrongFP,
            deviceId: nil,
            token: probe.token
        )
        var request = URLRequest(url: host.eventsURL)
        if let token = probe.token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let task = pinned.session.webSocketTask(with: request)
        let taskId = task.taskIdentifier
        task.resume()
        defer { task.cancel(with: .goingAway, reason: nil) }
        do {
            _ = try await task.receive()
            XCTFail("Expected WebSocket receive to fail with wrong fingerprint")
        } catch {
            XCTAssertTrue(
                pinned.consumeMismatch(taskIdentifier: taskId),
                "Expected PinnedSession to classify WSS failure as certificateChanged, got: \(error)"
            )
        }
    }

    func testGetAppInfoWithValidToken() async throws {
        let probe = try loadProbe()
        guard let token = probe.token else {
            throw XCTSkip("CODECK_PROBE has no token segment; skipping authenticated get_app_info check.")
        }
        let client = HostClient(address: probe.address, fingerprint: probe.fingerprint, token: token)
        try await client.checkAuth(timeout: 5)
    }

    func testGetAppInfoWithoutOrWrongTokenReturnsRemoved() async throws {
        let probe = try loadProbe()

        let noTokenClient = HostClient(address: probe.address, fingerprint: probe.fingerprint, token: nil)
        do {
            try await noTokenClient.checkAuth(timeout: 5)
            XCTFail("Expected 401 removed without token")
        } catch let error as HostError {
            XCTAssertEqual(error, .removed)
        }

        let wrongTokenClient = HostClient(
            address: probe.address,
            fingerprint: probe.fingerprint,
            token: "invalid-probe-token"
        )
        do {
            try await wrongTokenClient.checkAuth(timeout: 5)
            XCTFail("Expected 401 removed with wrong token")
        } catch let error as HostError {
            XCTAssertEqual(error, .removed)
        }
    }
}
