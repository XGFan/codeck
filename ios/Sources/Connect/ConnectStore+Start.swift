import Foundation

/// A Session to open (navigation), once.
struct OpenRequest: Equatable {
    let id = UUID()
    let sessionId: String
}

/// 新建 Session, Resume and Fork: start a connection on the Host, then follow it until claude names its Session.
extension ConnectStore {
    /// A Resume is on its way for this Session.
    func isResuming(_ sessionId: String) -> Bool { starts.values.contains(.resume(sessionId: sessionId)) }

    /// A Fork of this Session is on its way.
    func isForking(_ sessionId: String) -> Bool { starts.values.contains(.fork(origin: sessionId)) }

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
        errors[sessionId] = nil
        await dropEnded(of: sessionId)
        do {
            let conn = try await client.call(
                "ccrpc_resume", args: ["sessionId": sessionId, "cwd": cwd], as: String.self, timeout: 60
            )
            beginFollowing(conn, .resume(sessionId: sessionId))
            watchResumeTimeout(conn, sessionId: sessionId)
        } catch {
            errors[sessionId] = "Resume 失败：\(Self.text(error))"
        }
    }

    /// `ccrpc_fork` of the whole Session: its page opens once claude names the new Session.
    func fork(sessionId: String, cwd: String, title: String) async {
        guard let client, !isForking(sessionId) else { return }
        if errors[sessionId]?.hasPrefix("Fork 失败") == true { errors[sessionId] = nil }
        do {
            let conn = try await client.call(
                "ccrpc_fork", args: ForkArgs(sessionId: sessionId, cwd: cwd, title: title), as: String.self, timeout: 60
            )
            beginFollowing(conn, .fork(origin: sessionId))
        } catch {
            errors[sessionId] = "Fork 失败：\(Self.text(error))"
        }
    }

    private func beginFollowing(_ conn: String, _ kind: StartKind) {
        starts[conn] = kind
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
            return
        case .ready(let sessionId):
            starts[conn] = nil
            started[conn] = sessionId
            if case .fork = kind {
                if let titleError = conns[conn]?.fork?.titleError {
                    errors[sessionId] = "Fork 的标题没有改成：\(titleError)"
                }
                openRequest = OpenRequest(sessionId: sessionId)
            }
        case .failed(let reason):
            switch kind {
            case .new:
                starts[conn] = nil
                startFailures[conn] = reason
                ending.insert(conn)
            case .resume(let sessionId):
                // `starts` stays until the Pane is read and closed: 「Resume 中…」 until then.
                Task { await failResume(conn, sessionId: sessionId, timedOut: nil, fallback: reason) }
            case .fork(let origin):
                starts[conn] = nil
                errors[origin] = "Fork 失败：\(reason)"
                ending.insert(conn)
            }
        }
    }

    /// No Session named within 30 s (not counting time a card waits for an answer, e.g. the folder trust dialog).
    private func watchResumeTimeout(_ conn: String, sessionId: String) {
        Task {
            while true {
                try? await Task.sleep(for: .seconds(30))
                guard starts[conn] != nil else { return }
                if conns[conn]?.requests.isEmpty == false { continue }
                await failResume(conn, sessionId: sessionId, timedOut: "30 秒内没有启动", fallback: "claude 没有启动")
                return
            }
        }
    }

    /// Gives a Resume up: the reason (claude's last line on the Pane) goes on its Session, then the new Pane is
    /// closed and the connection dropped.
    private func failResume(_ conn: String, sessionId: String, timedOut: String?, fallback: String) async {
        // Once: the state and removal events its own exit and Disconnect cause come here too.
        guard starts[conn] != nil, !failingResumes.contains(conn) else { return }
        failingResumes.insert(conn)
        defer { failingResumes.remove(conn) }
        var line = fallback
        var paneNote = ""
        if conns[conn]?.procExit == nil {
            // Let the shell draw its prompt again, so claude's last line is above the cursor.
            try? await Task.sleep(for: .milliseconds(300))
            let screen = await command(conn, ["type": "get_screen"])
            if screen.success, case .array(let rows)? = screen.data?["lines"] {
                var cursor: Int?
                if case .array(let pos)? = screen.data?["cursor"], case .number(let row)? = pos.first { cursor = Int(row) }
                line = StartFlow.resumeFailure(rows.compactMap(\.stringValue), cursorRow: cursor)
            }
            // `exit` needs claude idle or gone: one still on a screen of its own keeps its Pane, say so.
            let e = await command(conn, ["type": "exit", "close_pane": true, "stop_tasks": true])
            if !e.success { paneNote = "；Pane 没有关掉：\(e.errorText)" }
        }
        errors[sessionId] = "Resume 失败：\(timedOut.map { "\($0)（\(line)）" } ?? line)\(paneNote)"
        starts[conn] = nil
        if let c = conns[conn] { await disconnect(c) } else { ending.insert(conn) }
    }

    private struct ForkArgs: Encodable, Sendable {
        var sessionId: String
        var cwd: String
        var title: String

        enum CodingKeys: String, CodingKey { case sessionId, cwd, at, restore, title }

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(sessionId, forKey: .sessionId)
            try c.encode(cwd, forKey: .cwd)
            // The whole Session: no prompt to fork before, nothing to restore.
            try c.encodeNil(forKey: .at)
            try c.encodeNil(forKey: .restore)
            try c.encode(title, forKey: .title)
        }
    }
}
