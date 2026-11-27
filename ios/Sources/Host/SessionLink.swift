import Foundation

/// `codeck://session/<Session ID>`: what the Host's push notifications open. The query is ignored.
enum SessionLink {
    /// The Session ID (lowercased, as the Host writes it), or nil when the URL is not a valid Session link.
    static func parse(_ url: URL) -> String? {
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              components.scheme?.lowercased() == "codeck",
              components.host?.lowercased() == "session",
              components.port == nil,
              components.user == nil,
              components.password == nil
        else { return nil }
        let parts = components.path.split(separator: "/", omittingEmptySubsequences: false)
        guard parts.count == 2, parts[0].isEmpty, let uuid = UUID(uuidString: String(parts[1])) else { return nil }
        return uuid.uuidString.lowercased()
    }
}
