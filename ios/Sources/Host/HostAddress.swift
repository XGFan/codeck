import Foundation

/// Where the Host's `codeck-host` daemon listens: a private IPv4 address and a port.
struct HostAddress: Equatable, Sendable {
    static let defaultPort = 38471

    var host: String
    var port: Int

    var baseURL: URL { URL(string: "http://\(host):\(port)")! }
    var display: String { "\(host):\(port)" }

    enum ValidationError: Error, Equatable, LocalizedError {
        case notIPv4
        case notPrivate
        case badPort

        var errorDescription: String? {
            switch self {
            case .notIPv4: "请输入 IPv4 地址，例如 192.168.1.20。"
            case .notPrivate: "只能连接局域网内的私网地址（10.x、172.16–31.x、192.168.x）。"
            case .badPort: "端口应为 1–65535 之间的整数。"
            }
        }
    }

    #if DEBUG
    static let allowsLoopback = true
    #else
    static let allowsLoopback = false
    #endif

    /// Validates user input. `hostInput` may carry its own `:port` (and a leading `http://`),
    /// which then wins over `portInput`.
    static func parse(
        host hostInput: String,
        port portInput: String,
        allowLoopback: Bool = allowsLoopback
    ) throws -> HostAddress {
        var host = hostInput.trimmingCharacters(in: .whitespacesAndNewlines)
        var portText = portInput.trimmingCharacters(in: .whitespacesAndNewlines)
        if host.hasPrefix("http://") { host.removeFirst("http://".count) }
        while host.hasSuffix("/") { host.removeLast() }
        if let colon = host.lastIndex(of: ":") {
            portText = String(host[host.index(after: colon)...])
            host = String(host[..<colon])
        }

        guard let octets = ipv4Octets(host) else { throw ValidationError.notIPv4 }
        let loopback = allowLoopback && octets == [127, 0, 0, 1]
        guard loopback || isPrivate(octets) else { throw ValidationError.notPrivate }

        let port = portText.isEmpty ? defaultPort : Int(portText)
        guard let port, (1...65535).contains(port), portText.allSatisfy(\.isASCII) else {
            throw ValidationError.badPort
        }
        return HostAddress(host: octets.map(String.init).joined(separator: "."), port: port)
    }

    /// Four decimal octets 0–255, nothing else.
    static func ipv4Octets(_ s: String) -> [Int]? {
        let parts = s.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return nil }
        var octets: [Int] = []
        for part in parts {
            guard (1...3).contains(part.count), part.allSatisfy({ $0.isASCII && $0.isNumber }),
                  let n = Int(part), n <= 255
            else { return nil }
            octets.append(n)
        }
        return octets
    }

    /// RFC 1918: 10/8, 172.16/12, 192.168/16.
    static func isPrivate(_ o: [Int]) -> Bool {
        guard o.count == 4 else { return false }
        return o[0] == 10 || (o[0] == 172 && (16...31).contains(o[1])) || (o[0] == 192 && o[1] == 168)
    }
}

/// The saved Host, in UserDefaults.
struct HostAddressStore {
    private static let hostKey = "codeck.host.address"
    private static let portKey = "codeck.host.port"

    var defaults: UserDefaults = .standard

    func load() -> HostAddress? {
        guard let host = defaults.string(forKey: Self.hostKey) else { return nil }
        let port = defaults.integer(forKey: Self.portKey)
        // Re-validate: a saved loopback address from a debug build is not used by a release build.
        return try? HostAddress.parse(host: host, port: String(port == 0 ? HostAddress.defaultPort : port))
    }

    func save(_ address: HostAddress) {
        defaults.set(address.host, forKey: Self.hostKey)
        defaults.set(address.port, forKey: Self.portKey)
    }
}
