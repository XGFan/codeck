import Foundation

/// `LiveStatus` in bindings.ts. An unknown value from a newer Host decodes as `.unknown`.
enum LiveStatus: String, Codable, Sendable {
    case busy, waiting, idle, unknown

    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = LiveStatus(rawValue: raw) ?? .unknown
    }

    /// Ordering in the Live section: waiting first, as on the desktop sidebar.
    var urgency: Int {
        switch self {
        case .waiting: 3
        case .busy: 2
        case .idle: 1
        case .unknown: 0
        }
    }
}

/// `LiveState` in bindings.ts.
struct LiveState: Codable, Hashable, Sendable {
    var status: LiveStatus
    var rawStatus: String?
    /// Only set while waiting (`permission prompt`, `input needed`, ...).
    var waitingFor: String?
    var pid: Int?
}

/// `SessionSummary` in bindings.ts (the subset the app shows).
struct SessionSummary: Codable, Hashable, Identifiable, Sendable {
    var id: String
    var projectId: String
    var title: String
    var createdMs: Double
    var lastActiveMs: Double
    var messageCount: Int
    var gitBranch: String?
    var live: LiveState?
    var primaryModel: String?
    var editedProjectIds: [String]?

    var lastActive: Date { Date(timeIntervalSince1970: lastActiveMs / 1000) }
}

/// `SessionDetail` in bindings.ts (the subset the app shows).
struct SessionDetail: Codable, Sendable {
    var summary: SessionSummary
    var cwd: String?
    var projectPath: String
    /// The Session this one is carried on in; the app opens that one instead.
    var supersededBy: String?
}

/// `SessionQuery` in bindings.ts.
struct SessionQuery: Encodable, Sendable {
    var projectIds: [String] = []
    var sort: String = "lastActive"
    var descending: Bool = true
    var timeRange: TimeRange? = nil

    struct TimeRange: Encodable, Sendable {
        var fromMs: Double?
        var toMs: Double?
    }

    static let all = SessionQuery()

    enum CodingKeys: String, CodingKey { case projectIds, sort, descending, timeRange, drill }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(projectIds, forKey: .projectIds)
        try c.encode(sort, forKey: .sort)
        try c.encode(descending, forKey: .descending)
        // The Host expects explicit nulls for the Option fields.
        if let timeRange { try c.encode(timeRange, forKey: .timeRange) } else { try c.encodeNil(forKey: .timeRange) }
        try c.encodeNil(forKey: .drill)
    }
}
