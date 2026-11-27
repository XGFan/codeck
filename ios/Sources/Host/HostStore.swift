import Foundation
import Security

/// A configured Host and its credentials. Non-loopback entries always carry a pinned TLS
/// fingerprint, `deviceId`, and Keychain `token`; only a DEBUG loopback entry may omit them.
struct PairedHost: Equatable, Sendable {
    var address: HostAddress
    var fingerprint: String?
    var deviceId: String?
    var token: String?

    var host: String { address.host }
    var port: Int { address.port }
    var isTLS: Bool { fingerprint != nil }
    var display: String { address.display }

    var baseURL: URL {
        URL(string: "\(isTLS ? "https" : "http")://\(address.host):\(address.port)")!
    }

    var eventsURL: URL {
        URL(string: "\(isTLS ? "wss" : "ws")://\(address.host):\(address.port)/v1/events")!
    }
}

enum KeychainError: Error, Equatable, LocalizedError {
    case unexpectedStatus(OSStatus)
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .unexpectedStatus(let status): "钥匙串保存失败（\(status)）。"
        case .failed(let message): message
        }
    }
}

/// Abstracts generic-password Keychain access so unit tests can inject an in-memory store.
protocol KeychainStoring: Sendable {
    func read(service: String, account: String) -> String?
    func write(_ value: String, service: String, account: String) throws
    func delete(service: String, account: String)
}

/// Keychain implementation backed by Security.framework (`AfterFirstUnlockThisDeviceOnly`).
struct SystemKeychain: KeychainStoring, Sendable {
    func read(service: String, account: String) -> String? {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess, let data = item as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    func write(_ value: String, service: String, account: String) throws {
        let data = Data(value.utf8)
        let addQuery: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
            kSecAttrAccessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData: data,
        ]
        let status = SecItemAdd(addQuery as CFDictionary, nil)
        if status == errSecDuplicateItem {
            let matchQuery: [CFString: Any] = [
                kSecClass: kSecClassGenericPassword,
                kSecAttrService: service,
                kSecAttrAccount: account,
            ]
            let update: [CFString: Any] = [
                kSecAttrAccessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
                kSecValueData: data,
            ]
            let updateStatus = SecItemUpdate(matchQuery as CFDictionary, update as CFDictionary)
            guard updateStatus == errSecSuccess else { throw KeychainError.unexpectedStatus(updateStatus) }
            return
        }
        guard status == errSecSuccess else { throw KeychainError.unexpectedStatus(status) }
    }

    func delete(service: String, account: String) {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
        ]
        SecItemDelete(query as CFDictionary)
    }
}

/// Persists the paired Host in UserDefaults (`codeck.host` JSON) and its bearer token in Keychain.
struct HostStore: @unchecked Sendable {
    static let storageKey = "codeck.host"
    static let legacyHostKey = "codeck.host.address"
    static let legacyPortKey = "codeck.host.port"
    static let keychainService = "dev.joy.codeck.host"

    struct Record: Codable, Equatable, Sendable {
        var host: String
        var port: Int
        var fingerprint: String?
        var deviceId: String?

        enum CodingKeys: String, CodingKey {
            case host, port, fingerprint, deviceId
        }

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(host, forKey: .host)
            try c.encode(port, forKey: .port)
            // Encode explicit nulls to match `{host, port, fingerprint|null, deviceId|null}`.
            try c.encode(fingerprint, forKey: .fingerprint)
            try c.encode(deviceId, forKey: .deviceId)
        }
    }

    let defaults: UserDefaults
    let keychain: any KeychainStoring
    let allowLoopback: Bool

    init(
        defaults: UserDefaults = .standard,
        keychain: any KeychainStoring = SystemKeychain(),
        allowLoopback: Bool = HostAddress.allowsLoopback
    ) {
        self.defaults = defaults
        self.keychain = keychain
        self.allowLoopback = allowLoopback
        // Legacy plaintext keys from before TLS pairing are dropped without migration.
        defaults.removeObject(forKey: Self.legacyHostKey)
        defaults.removeObject(forKey: Self.legacyPortKey)
    }

    func load() -> PairedHost? {
        guard let record = loadRecord(),
              let octets = HostAddress.ipv4Octets(record.host),
              (1...65535).contains(record.port)
        else { return nil }

        let normalizedHost = octets.map(String.init).joined(separator: ".")
        let address = HostAddress(host: normalizedHost, port: record.port)
        let isLoopback = octets == [127, 0, 0, 1]

        if isLoopback {
            // Only DEBUG builds may use an unencrypted loopback Host without fingerprint/token.
            guard allowLoopback, record.fingerprint == nil, record.deviceId == nil else { return nil }
            return PairedHost(address: address, fingerprint: nil, deviceId: nil, token: nil)
        }

        guard HostAddress.isPrivate(octets),
              let fingerprint = record.fingerprint,
              PairingLink.isBase64URL(fingerprint, length: PairingLink.fingerprintLength),
              let deviceId = record.deviceId, !deviceId.isEmpty,
              let token = keychain.read(service: Self.keychainService, account: deviceId), !token.isEmpty
        else { return nil }

        return PairedHost(address: address, fingerprint: fingerprint, deviceId: deviceId, token: token)
    }

    /// Saves a Host: for new credentials writes Keychain first (aborting without touching existing
    /// state on failure), then updates UserDefaults, then deletes any superseded Keychain item.
    /// Reusing the same fingerprint/deviceId/token updates only the address metadata in UserDefaults.
    func save(_ host: PairedHost) throws {
        let previousRecord = loadRecord()
        let previousDeviceId = previousRecord?.deviceId
        let reusingSameCredentials: Bool
        if host.fingerprint != nil,
           previousRecord?.fingerprint == host.fingerprint,
           let deviceId = host.deviceId, !deviceId.isEmpty,
           previousDeviceId == deviceId,
           let token = host.token, !token.isEmpty,
           keychain.read(service: Self.keychainService, account: deviceId) == token {
            reusingSameCredentials = true
        } else {
            reusingSameCredentials = false
        }

        if host.fingerprint != nil {
            guard let deviceId = host.deviceId, !deviceId.isEmpty,
                  let token = host.token, !token.isEmpty
            else {
                throw KeychainError.failed("缺少设备凭据。")
            }
            if !reusingSameCredentials {
                try keychain.write(token, service: Self.keychainService, account: deviceId)
            }
        } else {
            guard allowLoopback, host.address.host == "127.0.0.1" else {
                throw KeychainError.failed("非回环地址必须先完成配对。")
            }
        }

        let record = Record(
            host: host.address.host,
            port: host.address.port,
            fingerprint: host.fingerprint,
            deviceId: host.deviceId
        )
        let data = try JSONEncoder().encode(record)
        defaults.set(data, forKey: Self.storageKey)

        if let oldId = previousDeviceId, oldId != host.deviceId {
            keychain.delete(service: Self.keychainService, account: oldId)
        }
    }

    private func loadRecord() -> Record? {
        guard let data = defaults.data(forKey: Self.storageKey) else { return nil }
        return try? JSONDecoder().decode(Record.self, from: data)
    }
}
