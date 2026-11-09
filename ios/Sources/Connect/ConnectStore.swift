import Foundation
import Observation

/// The Host's Connects and `ccrpc ls` list, kept in step with its events. The Host holds the state; this only
/// mirrors it, sends commands, and remembers what is local to this app (errors shown, answers just sent).
@MainActor
@Observable
final class ConnectStore {
    /// By conn id.
    private(set) var conns: [String: ConnState] = [:]
    private(set) var ls: [LsEntry] = []
    /// `ccrpc_ls` failed (ccrpc not available on the Host).
    private(set) var lsError: String?
    /// The last failure to show on a Session's page, by Session id.
    var errors: [String: String] = [:]
    /// Sessions a Connect is on its way for.
    private(set) var connecting: Set<String> = []
    /// Requests answered here (`conn|request_id`): hidden at once, before the Host's state drops them.
    private var answered: Set<String> = []
    /// Connects this app is ending (Disconnect, 关闭 Session, a failed start): gone without a trace when the Host
    /// removes them.
    @ObservationIgnored var ending: Set<String> = []
    /// Connections started here (新建 Session, Resume, Fork) until claude names their Session, by conn id.
    var starts: [String: StartKind] = [:]
    /// What a started connection became: its Session id, by conn id (the 「正在启动…」 page turns into it).
    var started: [String: String] = [:]
    /// Why a 新建 Session failed (claude exited before naming its Session), by conn id.
    var startFailures: [String: String] = [:]
    /// A Session to open now (a Fork that just got its Session).
    var openRequest: OpenRequest?
    @ObservationIgnored var failingResumes: Set<String> = []
    /// Conn ids in the order they appeared, so the last ended one of a Session is well defined.
    private var order: [String] = []
    @ObservationIgnored private(set) var client: HostClient?

    /// A new Host: everything from the old one goes.
    func reset(client: HostClient?) {
        self.client = client
        conns = [:]
        order = []
        ls = []
        lsError = nil
        errors = [:]
        connecting = []
        answered = []
        ending = []
        starts = [:]
        started = [:]
        startFailures = [:]
        openRequest = nil
        failingResumes = []
    }

    // MARK: Snapshot and events

    /// After every `hello`: the Host keeps no event history, so take its whole state again.
    func resync() async {
        guard let client else { return }
        async let states = client.call("connect_states", args: Empty(), as: Lossy<ConnState>.self)
        async let list = client.call("ccrpc_ls", args: Empty(), as: Lossy<LsEntry>.self)
        if let states = try? await states, client.address == self.client?.address {
            let byId = Dictionary(states.items.map { ($0.conn, $0) }, uniquingKeysWith: { _, b in b })
            order = order.filter { byId[$0] != nil } + states.items.map(\.conn).filter { !order.contains($0) }
            conns = byId
            pruneAnswered()
            for conn in starts.keys { follow(conn, gone: byId[conn] == nil) }
        }
        do {
            let entries = try await list
            guard client.address == self.client?.address else { return }
            ls = entries.items
            lsError = nil
        } catch {
            guard client.address == self.client?.address else { return }
            lsError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// `connect-state-event`: the whole state of one Connect.
    func apply(_ state: ConnState) {
        if conns[state.conn] == nil, !order.contains(state.conn) { order.append(state.conn) }
        conns[state.conn] = state
        pruneAnswered()
        follow(state.conn)
    }

    /// `connect-removed-event` (after the final state, with `procExit`). One that ended by itself stays, so its
    /// page can say why; one this app ended goes.
    func remove(conn: String) {
        follow(conn, gone: true)
        if let c = conns[conn], !ending.contains(conn), !c.disconnected, c.procExit != nil || c.sessionExited {
            answered = answered.filter { !$0.hasPrefix(conn + "|") }
            return
        }
        ending.remove(conn)
        conns[conn] = nil
        order.removeAll { $0 == conn }
        answered = answered.filter { !$0.hasPrefix(conn + "|") }
    }

    /// Before a Resume: ended Connects of this Session go (one whose ccrpc still runs, driving nothing, is
    /// disconnected).
    func dropEnded(of sessionId: String) async {
        for c in conns.values where c.sessionId == sessionId && !c.isLive {
            if c.procExit == nil {
                await disconnect(c)
            } else {
                conns[c.conn] = nil
                order.removeAll { $0 == c.conn }
            }
        }
    }

    /// `ccrpc-ls-ended-event`: the Host's `ccrpc ls --watch` stopped; no list until the next one (or `hello`).
    func lsEnded(_ message: String) {
        ls = []
        lsError = message
    }

    /// `ccrpc-ls-event`: the JSON text `ccrpc ls --json` printed.
    func applyLs(_ text: String) {
        guard let entries = LsEntry.parseList(text) else { return }
        ls = entries
        lsError = nil
    }

    // MARK: Lookups

    /// The `ccrpc ls` entry of a Session (the Holder's session id is the Session id).
    func lsEntry(for sessionId: String) -> LsEntry? {
        ls.first { $0.sessionId == sessionId }
    }

    /// The Connect of a Session: a live one first, else the last that ended. Before ccrpc's `ready` it is known
    /// by its Holder only.
    func conn(for sessionId: String) -> ConnState? {
        let holder = lsEntry(for: sessionId)?.holderId
        let all = order.compactMap { conns[$0] }.filter { c in
            c.ready || c.sessionId != nil ? c.sessionId == sessionId : holder != nil && c.holderId == holder
        }
        return all.last { $0.isLive } ?? all.last
    }

    /// The Holder already has a live Connect (possibly to another Session after `/clear`).
    func holderConnected(_ holderId: String) -> Bool {
        conns.values.contains { $0.holderId == holderId && $0.isLive }
    }

    /// The cards to show: main line first, those just answered left out.
    func requests(of c: ConnState) -> [ConnRequest] {
        let open = c.requests.filter { !answered.contains("\(c.conn)|\($0.id)") }
        return open.filter { $0.agentId == nil } + open.filter { $0.agentId != nil }
    }

    private func pruneAnswered() {
        answered = answered.filter { key in
            conns.values.contains { c in c.requests.contains { "\(c.conn)|\($0.id)" == key } }
        }
    }

    // MARK: Commands

    func setError(_ sessionId: String, _ text: String?) {
        errors[sessionId] = text
    }

    /// 「Connect」: the Host attaches ccrpc to the Holder (or returns the Connect it already has).
    func connect(sessionId: String) async {
        guard let client, let entry = lsEntry(for: sessionId), !connecting.contains(sessionId) else { return }
        connecting.insert(sessionId)
        errors[sessionId] = nil
        defer { connecting.remove(sessionId) }
        do {
            let conn = try await client.call("connect_shared", args: ["holderId": entry.holderId], as: String.self)
            guard client.address == self.client?.address else { return }
            // Until its first state event: a placeholder, so the page shows 「启动中」 at once.
            if conns[conn] == nil {
                var c = ConnState(conn: conn)
                c.holderId = entry.holderId
                c.state = .starting
                apply(c)
            }
        } catch {
            errors[sessionId] = "连接失败：\(Self.text(error))"
        }
    }

    func disconnect(_ c: ConnState) async {
        guard let client else { return }
        ending.insert(c.conn)
        do {
            _ = try await client.call("ccrpc_disconnect", args: ["conn": c.conn], as: JSONValue.self)
        } catch {
            ending.remove(c.conn)
            if let id = c.sessionId { errors[id] = "Disconnect 失败：\(Self.text(error))" }
        }
    }

    /// One ccrpc command; a failure to reach the Host comes back as an unsuccessful response.
    func command(_ conn: String, _ command: JSONValue) async -> CommandResponse {
        guard let client else { return .failed("未连接 Host") }
        do {
            // The Host waits up to 150 s for ccrpc's response.
            return try await client.call(
                "connect_command",
                args: ConnectCommandArgs(conn: conn, command: command),
                as: CommandResponse.self,
                timeout: 160
            )
        } catch {
            var code: String?
            if case .host(let c, _) = error as? HostError { code = c }
            return .failed(Self.text(error), code: code)
        }
    }

    /// 「关闭 Session」 is under way: its ccrpc exiting next is not news.
    func markEnding(_ conn: String) {
        ending.insert(conn)
    }

    /// Answers a card; answered, it leaves at once (the Host's state drops it on the tool's result).
    func answer(_ c: ConnState, requestId: String, _ command: JSONValue) async -> CommandResponse {
        let r = await self.command(c.conn, command)
        if r.success { answered.insert("\(c.conn)|\(requestId)") }
        return r
    }

    /// Saves an image on the Host for a prompt's `parts`; returns its path there.
    func saveImage(jpeg: Data) async throws -> String {
        guard let client else { throw HostError.badResponse }
        return try await client.call(
            "ccrpc_save_image",
            args: ["base64": jpeg.base64EncodedString(), "ext": "jpg"],
            as: String.self,
            timeout: 60
        )
    }

    static func text(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }

    private struct Empty: Encodable, Sendable {}

    private struct ConnectCommandArgs: Encodable, Sendable {
        var conn: String
        var command: JSONValue
    }
}
