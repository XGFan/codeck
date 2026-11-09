import Foundation

/// `ProjectSummary` in bindings.ts.
struct ProjectSummary: Codable, Hashable, Identifiable, Sendable {
    var id: String
    var path: String
    var displayName: String
    /// The project directory no longer exists on disk.
    var missing: Bool?
    var sessionCount: Int?
    var lastActiveMs: Double?
}

/// `SidebarProject` in bindings.ts: a Project added to the desktop's sidebar.
struct SidebarProject: Codable, Hashable, Sendable {
    var path: String
    var nest: Bool?
}
