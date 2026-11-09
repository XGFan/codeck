import Foundation

// 新建 Session, Resume and Fork (desktop: resume.ts, fork.ts, the store's newSession / resume / fork).

/// What a Session page offers for Resume or Fork.
enum StartOffer: Equatable {
    case hidden
    case enabled
    case disabled(String)

    /// Resume is for an ended Session: one still running (in clay or a terminal) is not started a second time,
    /// which would have two claude processes write one transcript. It needs the directory the Session started in,
    /// and ccrpc.
    static func resume(live: Bool, cwd: String?, cwdMissing: Bool, ccrpcError: String?) -> StartOffer {
        if live { return .hidden }
        guard let cwd, !cwd.isEmpty else { return .disabled("不知道这个 Session 的启动目录") }
        if cwdMissing { return .disabled("目录 \(cwd) 已不存在") }
        if let ccrpcError { return .disabled(ccrpcError) }
        return .enabled
    }

    /// Like Resume's, but also for a Session still running (a Fork never touches it).
    static func fork(cwd: String?, cwdMissing: Bool, ccrpcError: String?) -> StartOffer {
        resume(live: false, cwd: cwd, cwdMissing: cwdMissing, ccrpcError: ccrpcError)
    }
}

/// A connection started here, followed until claude names its Session.
enum StartKind: Equatable, Sendable {
    case new(cwd: String)
    /// Continues this Session (the id stays).
    case resume(sessionId: String)
    /// A new Session copying `origin`.
    case fork(origin: String)
}

enum StartFlow {
    enum Outcome: Equatable {
        case waiting
        /// claude named the Session.
        case ready(sessionId: String)
        /// It ended before that; why, in one line.
        case failed(String)
    }

    /// Where a started connection is: its state has a Session id (a Fork: its `fork` event came), it ended first,
    /// or neither yet.
    static func outcome(_ kind: StartKind, _ c: ConnState?) -> Outcome {
        guard let c else { return .waiting }
        if case .fork = kind {
            if let f = c.fork {
                if let e = f.error { return .failed(e.message ?? e.code ?? "") }
                if let id = f.sessionId { return .ready(sessionId: id) }
            }
        } else if let id = c.sessionId {
            return .ready(sessionId: id)
        }
        return c.isLive ? .waiting : .failed(reason(c))
    }

    /// Why a connection ended: ccrpc's last stderr line, else its last notice.
    static func reason(_ c: ConnState) -> String {
        ConnectWords.lastLine(c.procExit?.stderr) ?? c.notices.last?.text
            ?? c.procExit.map(ConnectWords.procExit) ?? "claude 退出了"
    }

    /// Why a Resume failed, from the Pane's screen once claude exited: the last error line claude printed, else the
    /// last non-empty line above the cursor (the shell's prompt that came back starts below it).
    static func resumeFailure(_ lines: [String], cursorRow: Int?) -> String {
        let rows = lines.map { $0.trimmingCharacters(in: .whitespaces) }
        let isError = { (l: String) in
            l.range(of: #"^(No conversation found|Error:|error:)"#, options: .regularExpression) != nil
        }
        if let error = rows.last(where: isError) { return error }
        let above = (cursorRow.map { Array(rows.prefix(max(0, $0))) } ?? Array(rows.dropLast())).filter { !$0.isEmpty }
        return above.last ?? rows.last { !$0.isEmpty } ?? "claude 退出了"
    }

    /// The Fork's title: the Origin Session's (on one line, without its own `(Fork)` / `(Fork N)`), then
    /// ` (Fork)`, or ` (Fork 2)`, ` (Fork 3)`… when a Session already has that title.
    static func forkTitle(_ origin: String, taken: some Sequence<String>) -> String {
        let oneLine = origin.replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespaces)
        let base = oneLine.replacingOccurrences(of: #"\s*\(Fork(?: \d+)?\)$"#, with: "", options: .regularExpression)
        let used = Set(taken)
        let at = { (n: Int) in "\(base) (Fork\(n > 1 ? " \(n)" : ""))".trimmingCharacters(in: .whitespaces) }
        var n = 1
        while used.contains(at(n)) { n += 1 }
        return at(n)
    }
}

extension StartFlow {
    struct Directory: Equatable, Identifiable {
        var path: String
        var name: String
        var id: String { path }
    }

    /// Where a new Session can start: the desktop sidebar's Projects first, then the indexed Projects by recency;
    /// missing directories and repeats left out.
    static func directories(sidebar: [SidebarProject]?, projects: [ProjectSummary]) -> [Directory] {
        var seen = Set<String>()
        var out: [Directory] = []
        let named = Dictionary(projects.map { ($0.path, $0.displayName) }, uniquingKeysWith: { a, _ in a })
        let missing = Set(projects.filter { $0.missing == true }.map(\.path))
        for p in sidebar ?? [] where !missing.contains(p.path) && seen.insert(p.path).inserted {
            out.append(Directory(path: p.path, name: named[p.path] ?? (p.path as NSString).lastPathComponent))
        }
        for p in projects.sorted(by: { ($0.lastActiveMs ?? 0) > ($1.lastActiveMs ?? 0) })
        where p.missing != true && seen.insert(p.path).inserted {
            out.append(Directory(path: p.path, name: p.displayName))
        }
        return out
    }

    /// The directory typed, as `ccrpc_new` takes it: trimmed, without a trailing `/`.
    static func cwd(from typed: String) -> String {
        var s = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        while s.count > 1, s.hasSuffix("/") { s.removeLast() }
        return s
    }

    /// Why a typed directory cannot start a Session: the Host takes full paths only (it does not expand `~`).
    /// nil when it can, or when nothing is typed.
    static func cwdProblem(_ typed: String) -> String? {
        let cwd = cwd(from: typed)
        return cwd.isEmpty || cwd.hasPrefix("/") ? nil : "请输入以 / 开头的完整路径"
    }
}
