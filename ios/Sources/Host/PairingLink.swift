import Foundation

/// Parsed `codeck://pair?h=<IPv4>[,<IPv4>…]&p=<port>&fp=<43-char base64url>&k=<22-char base64url>`.
struct PairingLink: Equatable, Sendable {
    static let fingerprintLength = 43
    static let keyLength = 22
    static let maxHosts = 4

    var hosts: [String]
    var port: Int
    var fingerprint: String
    var key: String

    var addresses: [HostAddress] { hosts.map { HostAddress(host: $0, port: port) } }
    var firstHost: String { hosts[0] }

    enum ParseError: Error, Equatable, LocalizedError {
        case invalid

        var errorDescription: String? {
            switch self {
            case .invalid: "这不是 CoDeck 的配对二维码。"
            }
        }
    }

    static func parse(_ string: String) throws -> PairingLink {
        let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let components = URLComponents(string: trimmed) else {
            throw ParseError.invalid
        }
        return try parse(components)
    }

    /// Returns true for `codeck://pair` URLs regardless of whether their path or query is valid.
    static func isPairingURL(_ url: URL) -> Bool {
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            return false
        }
        return components.scheme?.lowercased() == "codeck"
            && components.host?.lowercased() == "pair"
    }

    static func parse(_ url: URL) throws -> PairingLink {
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            throw ParseError.invalid
        }
        return try parse(components)
    }

    private static func parse(_ components: URLComponents) throws -> PairingLink {
        guard components.scheme?.lowercased() == "codeck",
              components.host?.lowercased() == "pair",
              components.path.isEmpty,
              components.port == nil,
              components.user == nil,
              components.password == nil,
              let queryItems = components.queryItems
        else {
            throw ParseError.invalid
        }

        let requiredKeys: Set<String> = ["h", "p", "fp", "k"]
        var params: [String: String] = [:]
        for item in queryItems where requiredKeys.contains(item.name) {
            // Reject duplicate or empty required parameters.
            guard params[item.name] == nil, let value = item.value, !value.isEmpty else {
                throw ParseError.invalid
            }
            params[item.name] = value
        }

        guard let rawHosts = params["h"],
              let rawPort = params["p"],
              let fp = params["fp"],
              let key = params["k"]
        else {
            throw ParseError.invalid
        }

        // Only RFC 1918 private IPv4 addresses are accepted, even in DEBUG builds.
        let parts = rawHosts.split(separator: ",", omittingEmptySubsequences: false)
        guard !parts.isEmpty, parts.count <= maxHosts else { throw ParseError.invalid }
        var hosts: [String] = []
        hosts.reserveCapacity(parts.count)
        for part in parts {
            guard let octets = HostAddress.ipv4Octets(String(part)), HostAddress.isPrivate(octets) else {
                throw ParseError.invalid
            }
            hosts.append(octets.map(String.init).joined(separator: "."))
        }

        guard rawPort.allSatisfy({ $0.isASCII && $0.isNumber }),
              let port = Int(rawPort),
              (1...65535).contains(port)
        else {
            throw ParseError.invalid
        }

        guard isBase64URL(fp, length: fingerprintLength),
              isBase64URL(key, length: keyLength)
        else {
            throw ParseError.invalid
        }

        return PairingLink(hosts: hosts, port: port, fingerprint: fp, key: key)
    }

    /// Unpadded base64url alphabet (`A-Z`, `a-z`, `0-9`, `-`, `_`) with exact length.
    static func isBase64URL(_ value: String, length: Int) -> Bool {
        value.utf8.count == length && value.utf8.allSatisfy { b in
            (b >= 65 && b <= 90) || (b >= 97 && b <= 122) || (b >= 48 && b <= 57) || b == 45 || b == 95
        }
    }
}
