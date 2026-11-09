import Foundation

// The Host's Connect state (connect-wire-contract, apiVersion 1). The outer ConnState is camelCase; what ccrpc
// sends as is (requests, tasks, waiting, progress) stays snake_case. Decoding is lenient: unknown keys are
// ignored, missing ones take a default, and an element that does not decode is dropped or kept as `.other`.

/// One Connect on the Host.
struct ConnState: Decodable, Equatable, Sendable {
    enum Phase: String, Sendable {
        case starting, idle, busy, waiting, exited
    }

    struct Waiting: Decodable, Equatable, Sendable {
        var kind: String
        var waitingFor: String?
        enum CodingKeys: String, CodingKey { case kind, waitingFor = "waiting_for" }
    }

    struct Progress: Decodable, Equatable, Sendable {
        var text: String?
        var verb: String?
        var elapsedS: Double?
        var tokens: Double?
        enum CodingKeys: String, CodingKey { case text, verb, elapsedS = "elapsed_s", tokens }
    }

    struct ProcExit: Decodable, Equatable, Sendable {
        var code: Int?
        var stderr: String?
    }

    /// The last turn's end (ccrpc's `turn_end`), the part this app reads.
    struct LastTurn: Decodable, Equatable, Sendable {
        var durationMs: Double?
        var aborted: Bool?
        enum CodingKeys: String, CodingKey { case durationMs = "duration_ms", aborted }
    }

    /// ccrpc's `fork` event, as is: how a Fork's setup ended.
    struct ForkResult: Decodable, Equatable, Sendable {
        struct Failure: Decodable, Equatable, Sendable {
            var code: String?
            var message: String?
        }

        /// The new Session (success).
        var sessionId: String?
        var error: Failure?
        /// Renaming the Fork to its title failed (it still succeeded).
        var titleError: String?

        enum CodingKeys: String, CodingKey { case sessionId = "session_id", error, titleError = "title_error" }

        init(sessionId: String? = nil, error: Failure? = nil, titleError: String? = nil) {
            self.sessionId = sessionId
            self.error = error
            self.titleError = titleError
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            sessionId = c.lenient(String.self, .sessionId)
            error = c.lenient(Failure.self, .error)
            titleError = c.lenient(String.self, .titleError)
        }
    }

    struct Notice: Decodable, Equatable, Sendable {
        var id: Int
        var text: String
    }

    var conn: String
    var holderId: String?
    var sessionId: String?
    var cwd: String?
    var claudeVersion: String?
    var ready = false
    /// `nil` for a state word this app does not know.
    var state: Phase?
    var waiting: Waiting?
    var mode: String?
    var model: String?
    var effort: String?
    var contextTokens: Double?
    var requests: [ConnRequest] = []
    var progress: Progress?
    var queue: [String] = []
    var tasks: [ConnTask] = []
    var lastTurn: LastTurn?
    var procExit: ProcExit?
    var sessionExited = false
    var notices: [Notice] = []
    /// The `fork` event of a Fork's connection (nil until it comes; absent from older Hosts).
    var fork: ForkResult?
    /// A client of this Host called `ccrpc_disconnect` on it: its end is not a failure.
    var disconnected = false

    enum CodingKeys: String, CodingKey {
        case conn, holderId, sessionId, cwd, claudeVersion, ready, state, waiting, mode, model, effort, contextTokens
        case requests, progress, queue, tasks, lastTurn, procExit, sessionExited, notices, fork, disconnected
    }

    init(conn: String) { self.conn = conn }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        conn = try c.decode(String.self, forKey: .conn)
        holderId = c.lenient(String.self, .holderId)
        sessionId = c.lenient(String.self, .sessionId)
        cwd = c.lenient(String.self, .cwd)
        claudeVersion = c.lenient(String.self, .claudeVersion)
        ready = c.lenient(Bool.self, .ready) ?? false
        state = c.lenient(String.self, .state).flatMap(Phase.init(rawValue:))
        waiting = c.lenient(Waiting.self, .waiting)
        mode = c.lenient(String.self, .mode)
        model = c.lenient(String.self, .model)
        effort = c.lenient(String.self, .effort)
        contextTokens = c.lenient(Double.self, .contextTokens)
        requests = c.lenient(Lossy<ConnRequest>.self, .requests)?.items ?? []
        progress = c.lenient(Progress.self, .progress)
        queue = c.lenient(Lossy<String>.self, .queue)?.items ?? []
        tasks = c.lenient(Lossy<ConnTask>.self, .tasks)?.items ?? []
        lastTurn = c.lenient(LastTurn.self, .lastTurn)
        procExit = c.lenient(ProcExit.self, .procExit)
        sessionExited = c.lenient(Bool.self, .sessionExited) ?? false
        notices = c.lenient(Lossy<Notice>.self, .notices)?.items ?? []
        fork = c.lenient(ForkResult.self, .fork)
        disconnected = c.lenient(Bool.self, .disconnected) ?? false
    }

    /// Still on its Session: ccrpc runs and the Session has not ended.
    var isLive: Bool { procExit == nil && !sessionExited }

    /// Commands can be sent: live and past ccrpc's `ready`.
    var canCommand: Bool { isLive && ready }

    var isBusy: Bool { state == .busy || state == .waiting }

    var runningTasks: [ConnTask] { tasks.filter { $0.status == "running" } }
}

/// A background task (`get_tasks`).
struct ConnTask: Decodable, Equatable, Identifiable, Sendable {
    var id: String
    var kind: String
    var toolUseId: String?
    var description: String?
    var command: String?
    var status: String

    enum CodingKeys: String, CodingKey { case id, kind, toolUseId = "tool_use_id", description, command, status }

    init(id: String, kind: String, description: String? = nil, command: String? = nil, status: String) {
        self.id = id
        self.kind = kind
        self.description = description
        self.command = command
        self.status = status
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = c.lenient(String.self, .kind) ?? "shell"
        toolUseId = c.lenient(String.self, .toolUseId)
        description = c.lenient(String.self, .description)
        command = c.lenient(String.self, .command)
        status = c.lenient(String.self, .status) ?? "running"
    }

    /// What the list shows as its name.
    var name: String { [description, command].compactMap { $0 }.first { !$0.isEmpty } ?? id }
}

/// One entry of `ccrpc ls --json`: a Holder (a clay Pane running claude with ccrpc's plugin).
struct LsEntry: Decodable, Equatable, Sendable {
    var holderId: String
    var sessionId: String?
    var status: String?
    var cwd: String?
    var plugin = false
    var hidden: Bool?
    var title: String?

    enum CodingKeys: String, CodingKey {
        case holderId = "holder_id", sessionId = "session_id", status, cwd, plugin, hidden, title
    }

    init(holderId: String, sessionId: String?, plugin: Bool) {
        self.holderId = holderId
        self.sessionId = sessionId
        self.plugin = plugin
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        holderId = try c.decode(String.self, forKey: .holderId)
        sessionId = c.lenient(String.self, .sessionId)
        status = c.lenient(String.self, .status)
        cwd = c.lenient(String.self, .cwd)
        plugin = c.lenient(Bool.self, .plugin) ?? false
        hidden = c.lenient(Bool.self, .hidden)
        title = c.lenient(String.self, .title)
    }

    /// `ccrpc-ls-event`'s payload is the JSON text `ccrpc ls --json` printed.
    static func parseList(_ text: String) -> [LsEntry]? {
        (try? JSONDecoder().decode(Lossy<LsEntry>.self, from: Data(text.utf8)))?.items
    }
}

/// ccrpc's response to a command, as `connect_command` returns it.
struct CommandResponse: Decodable, Equatable, Sendable {
    struct Failure: Decodable, Equatable, Sendable {
        var code: String?
        var message: String?
    }

    var command: String?
    var success: Bool
    var data: JSONValue?
    var error: Failure?

    /// A failure that never reached ccrpc (the Host refused it, or could not be reached).
    static func failed(_ message: String, code: String? = nil) -> CommandResponse {
        CommandResponse(command: nil, success: false, data: nil, error: Failure(code: code, message: message))
    }

    /// The error as the user reads it after 「…失败：」.
    var errorText: String { error?.message.flatMap { $0.isEmpty ? nil : $0 } ?? error?.code ?? "" }
}

// MARK: Lenient decoding

/// An array whose elements that fail to decode are skipped.
struct Lossy<T: Decodable>: Decodable {
    var items: [T]

    init(from decoder: Decoder) throws {
        var c = try decoder.unkeyedContainer()
        var items: [T] = []
        while !c.isAtEnd {
            if let item = try? c.decode(T.self) {
                items.append(item)
            } else if (try? c.decode(JSONValue.self)) == nil {
                break
            }
        }
        self.items = items
    }
}

extension KeyedDecodingContainer {
    /// The value, or nil when it is missing, null, or of another type.
    func lenient<T: Decodable>(_: T.Type, _ key: Key) -> T? {
        (try? decodeIfPresent(T.self, forKey: key)) ?? nil
    }
}
