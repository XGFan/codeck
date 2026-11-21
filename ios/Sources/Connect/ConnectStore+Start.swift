import Foundation

/// A Session to open (navigation), once.
struct OpenRequest: Equatable {
    let id = UUID()
    let sessionId: String
}

/// 新建 Session, Resume and Fork: start a connection on the Host, then follow it until claude names its Session.
extension ConnectStore {
    /// A Resume is on its way for this Session.
    func isResuming(_ sessionId: String) -> Bool { isStarting(.resume(sessionId: sessionId)) }

    /// A Fork of this Session is on its way.
    func isForking(_ sessionId: String) -> Bool { isStarting(.fork(origin: sessionId)) }

    private func isStarting(_ kind: StartKind) -> Bool { pendingStarts.contains(kind) || starts.values.contains(kind) }

    /// `ccrpc_new {cwd}`: returns the connection to follow (its page shows 「正在启动…」).
    func startNew(cwd: String) async throws -> String {
        guard let client else { throw HostError.badResponse }
        let conn = try await client.call("ccrpc_new", args: ["cwd": cwd], as: String.self, timeout: 60)
        beginFollowing(conn, .new(cwd: cwd))
        return conn
    }

    /// `ccrpc_resume {sessionId, cwd}`: the page stays on the Session, which the connection takes once ready.
    func resume(sessionId: String, cwd: String) async {
        guard let client, !isResuming(sessionId) else { return }
        let kind = StartKind.resume(sessionId: sessionId)
        pendingStarts.insert(kind)
        defer { pendingStarts.remove(kind) }
        errors[sessionId] = nil
        await dropEnded(of: sessionId)
        do {
            let conn = try await client.call(
                "ccrpc_resume", args: ["sessionId": sessionId, "cwd": cwd], as: String.self, timeout: 60
            )
            beginFollowing(conn, kind)
        } catch {
            errors[sessionId] = "Resume 失败：\(Self.text(error))"
        }
    }

    /// `ccrpc_fork`: the whole Session, or up to before its Main Line prompt `at` with `restore` (`conversation`,
    /// or `both` for the code too). Its page opens once claude names the new Session.
    func fork(sessionId: String, cwd: String, title: String, at: String? = nil, restore: String? = nil) async {
        guard let client, !isForking(sessionId) else { return }
        let kind = StartKind.fork(origin: sessionId)
        pendingStarts.insert(kind)
        defer { pendingStarts.remove(kind) }
        if errors[sessionId]?.hasPrefix("Fork 失败") == true { errors[sessionId] = nil }
        do {
            let conn = try await client.call(
                "ccrpc_fork",
                args: ForkArgs(sessionId: sessionId, cwd: cwd, at: at, restore: restore, title: title),
                as: String.self,
                timeout: 60
            )
            beginFollowing(conn, kind)
        } catch {
            errors[sessionId] = "Fork 失败：\(Self.text(error))"
        }
    }

    private func beginFollowing(_ conn: String, _ kind: StartKind) {
        starts[conn] = kind
        // Newer than a snapshot on its way, which may not know this connection yet.
        touched.insert(conn)
        // Its state may have come before the call returned.
        follow(conn)
    }

    /// Checks a started connection against its latest state (`gone`: the Host dropped it).
    func follow(_ conn: String, gone: Bool = false) {
        guard let kind = starts[conn] else { return }
        var outcome = StartFlow.outcome(kind, conns[conn])
        if gone, outcome == .waiting { outcome = .failed(conns[conn].map(StartFlow.reason) ?? "claude 退出了") }
        switch outcome {
        case .waiting:
            // A card waits on the Fork before it is done: answered on its Session, as on the desktop.
            if case .fork(let origin) = kind, let c = conns[conn], let id = c.sessionId, !c.requests.isEmpty,
               openPages[origin] != nil, forkCardsOpened.insert(conn).inserted {
                openRequest = OpenRequest(sessionId: id)
            }
            return
        case .ready(let sessionId):
            starts[conn] = nil
            started[conn] = sessionId
            if case .fork = kind {
                if let titleError = conns[conn]?.fork?.titleError {
                    errors[sessionId] = "Fork 的标题没有改成：\(titleError)"
                }
                // The prompt forked before goes into the Fork's input box here, as on the desktop.
                if let text = conns[conn]?.fork?.text, !text.isEmpty { restoredPrompts[conn] = text }
                if forkCardsOpened.remove(conn) == nil { openRequest = OpenRequest(sessionId: sessionId) }
            }
        case .failed(let reason):
            // ccrpc now says why itself (`start_failed`, `fatal`) and then closes that Pane and ends: the Host
            // removes the connection, and nothing here has to end it.
            let told = conns[conn].map { $0.startFailed != nil || $0.fatal != nil } ?? false
            switch kind {
            case .new:
                starts[conn] = nil
                startFailures[conn] = reason
                if !told { endFailedStart(conn) }
            case .resume(let sessionId):
                starts[conn] = nil
                errors[sessionId] = "Resume 失败：\(reason)"
                if !told { endFailedStart(conn) }
            case .fork(let origin):
                starts[conn] = nil
                errors[origin] = "Fork 失败：\(reason)"
                // Its page was opened for a card: the reason goes there too.
                if forkCardsOpened.remove(conn) != nil, let id = conns[conn]?.sessionId { errors[id] = "Fork 失败：\(reason)" }
                if !told { endFailedStart(conn) }
            }
        }
    }

    /// A start that failed: its connection goes, and a ccrpc still running on nothing is ended (it does not exit
    /// by itself once claude is gone; one that is already exiting makes this call fail, which says nothing).
    private func endFailedStart(_ conn: String) {
        ending.insert(conn)
        guard let c = conns[conn], c.procExit == nil, let client else { return }
        Task { _ = try? await client.call("ccrpc_disconnect", args: ["conn": conn], as: JSONValue.self) }
    }

    struct ForkArgs: Encodable, Sendable {
        var sessionId: String
        var cwd: String
        /// Nil for the whole Session: no prompt to fork before, nothing to restore.
        var at: String?
        var restore: String?
        var title: String

        enum CodingKeys: String, CodingKey { case sessionId, cwd, at, restore, title }

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(sessionId, forKey: .sessionId)
            try c.encode(cwd, forKey: .cwd)
            // The Host's command takes every argument: null, not left out.
            try c.encode(at, forKey: .at)
            try c.encode(restore, forKey: .restore)
            try c.encode(title, forKey: .title)
        }
    }
}
