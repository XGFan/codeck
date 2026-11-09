import Foundation

/// `GET /health`.
struct HostHealth: Decodable, Sendable {
    var product: String
    var apiVersion: Int
    var version: String?
}

/// `SessionsChanged` in bindings.ts.
struct SessionsChanged: Decodable, Sendable {
    var changed: [String]
    var removed: [String]
    var projectsChanged: Bool
}

/// `LiveEntry` in bindings.ts.
struct LiveEntry: Decodable, Sendable {
    var sessionId: String
    var state: LiveState
}

/// `LiveChanged` in bindings.ts: a full snapshot of the Live Session set.
struct LiveChanged: Decodable, Sendable {
    var live: [LiveEntry]
}

/// One message on `/v1/events`, decoded by `event` name.
enum HostEvent: Sendable {
    case hello(apiVersion: Int)
    case sessionsChanged(SessionsChanged)
    case liveChanged(LiveChanged)
    case indexStatus
    case pricesChanged
    /// One Connect's whole state changed.
    case connectState(ConnState)
    /// A Connect is gone (its ccrpc exited and was cleaned up).
    case connectRemoved(conn: String)
    /// `ccrpc ls --json`'s new list, as the JSON text it printed.
    case ccrpcLs(String)
    /// The Host's `ccrpc ls --watch` stopped, and why.
    case ccrpcLsEnded(message: String)
    /// An event this version of the app does not use (later additions).
    case other(String)

    private struct Envelope: Decodable { var event: String }
    private struct Payload<T: Decodable>: Decodable { var payload: T }
    private struct Hello: Decodable { var apiVersion: Int }
    private struct Removed: Decodable { var conn: String }
    private struct Ls: Decodable { var ls: String }
    private struct LsEnded: Decodable { var message: String }

    static func decode(_ data: Data) throws -> HostEvent {
        let decoder = JSONDecoder()
        let name = try decoder.decode(Envelope.self, from: data).event
        switch name {
        case "hello":
            return .hello(apiVersion: try decoder.decode(Payload<Hello>.self, from: data).payload.apiVersion)
        case "sessions-changed-event":
            return .sessionsChanged(try decoder.decode(Payload<SessionsChanged>.self, from: data).payload)
        case "live-changed-event":
            return .liveChanged(try decoder.decode(Payload<LiveChanged>.self, from: data).payload)
        case "index-status-event":
            return .indexStatus
        case "prices-changed-event":
            return .pricesChanged
        case "connect-state-event":
            return .connectState(try decoder.decode(Payload<ConnState>.self, from: data).payload)
        case "connect-removed-event":
            return .connectRemoved(conn: try decoder.decode(Payload<Removed>.self, from: data).payload.conn)
        case "ccrpc-ls-event":
            return .ccrpcLs(try decoder.decode(Payload<Ls>.self, from: data).payload.ls)
        case "ccrpc-ls-ended-event":
            return .ccrpcLsEnded(message: try decoder.decode(Payload<LsEnded>.self, from: data).payload.message)
        default:
            return .other(name)
        }
    }
}
