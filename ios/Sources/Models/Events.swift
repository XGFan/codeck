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
    /// An event this version of the app does not use (Connect events, later additions).
    case other(String)

    private struct Envelope: Decodable { var event: String }
    private struct Payload<T: Decodable>: Decodable { var payload: T }
    private struct Hello: Decodable { var apiVersion: Int }

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
        default:
            return .other(name)
        }
    }
}
