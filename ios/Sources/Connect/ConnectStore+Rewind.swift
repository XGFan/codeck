import Foundation

/// Where a conversation was rewound to: the transcript changes only with the next prompt, so until then the page
/// marks it from this prompt on.
struct Rewound: Equatable, Sendable {
    /// The Session it was rewound in (the Connect may go on to another).
    var sessionId: String?
    /// The prompt's transcript uuid.
    var uuid: String
    /// The code was restored too.
    var withCode: Bool
}

/// Rewind (the desktop store's `rewind` / `answerRewind`): 「回退」 on a prompt opens the terminal's confirm page,
/// which the Host's state shows as a card; answering it restores the conversation, the code, or both.
extension ConnectStore {
    /// The Connect of a Session while it takes commands (a Resume on its way does not yet).
    func liveConn(for sessionId: String) -> ConnState? {
        guard let c = conn(for: sessionId), c.canCommand, !isResuming(sessionId) else { return nil }
        return c
    }

    /// Where `c`'s conversation was rewound to, while it is still on that Session.
    func rewound(of c: ConnState) -> Rewound? {
        guard c.isLive, let r = rewounds[c.conn], r.sessionId == c.sessionId else { return nil }
        return r
    }

    /// 「回退」 on the prompt `uuid`: a failure is shown on the Session.
    func rewind(_ c: ConnState, uuid: String) async {
        guard c.canCommand, c.state == .idle, rewinding[c.conn] == nil else { return }
        if let id = c.sessionId { errors[id] = nil }
        rewinding[c.conn] = uuid
        let r = await command(c.conn, ["type": "rewind", "uuid": .string(uuid)])
        if r.success {
            // The card comes with the Host's next state, a moment after this response. Until it is there the input
            // box's 「中止」 stays off: its Esc would only step the terminal back into the Rewind picker.
            for _ in 0..<20 where !hasRewindCard(c.conn) { try? await Task.sleep(for: .milliseconds(100)) }
        } else if let id = c.sessionId {
            errors[id] = "回退失败：\(r.errorText)"
        }
        rewinding[c.conn] = nil
    }

    private func hasRewindCard(_ conn: String) -> Bool {
        conns[conn]?.requests.contains { if case .rewind = $0 { true } else { false } } ?? false
    }

    /// Answers the confirm page (`nil`: cancel); nil when done, else what to say on the card. Restoring the
    /// conversation puts the prompt into the input box and marks where the conversation now ends.
    func answerRewind(_ c: ConnState, _ request: RewindRequest, _ option: RewindRequest.Option?) async -> String? {
        var cmd: [String: JSONValue] = ["type": "answer_rewind", "request_id": .string(request.requestId)]
        if let option { cmd["option"] = .string(option.rawValue) } else { cmd["cancel"] = true }
        // A Fork's own rewind: ccrpc goes on with the Fork, whose `fork` event brings the prompt.
        let forking = { if case .fork = self.starts[c.conn] { true } else { false } }()
        let r = await answer(c, requestId: request.requestId, .object(cmd))
        guard r.success else {
            // The page is gone (answered or left in the terminal): so is its card. A Fork's still waits in ccrpc.
            if !forking, ["unknown_request", "not_waiting"].contains(r.error?.code) {
                hideRequest(c, requestId: request.requestId)
            }
            return "\(option == nil ? "取消回退" : "回退")失败：\(r.errorText)"
        }
        guard let option, !forking else { return nil }
        if option == .code {
            notes[c.conn] = "已恢复代码"
        } else {
            if let text = r.data?["text"]?.stringValue, !text.isEmpty { restoredPrompts[c.conn] = text }
            rewounds[c.conn] = Rewound(sessionId: c.sessionId, uuid: request.uuid, withCode: option == .both)
        }
        return nil
    }
}
