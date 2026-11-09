import XCTest
@testable import CoDeck

/// ConnState as the Host sends it (connect-wire-contract): camelCase outside, ccrpc's snake_case inside.
final class ConnStateDecodingTests: XCTestCase {
    static let stateJSON = """
    {
      "conn": "c1", "holderId": "h1", "sessionId": "s1", "cwd": "/repo", "claudeVersion": "2.1.0",
      "ready": true, "state": "waiting", "waiting": {"kind": "permission", "waiting_for": "permission prompt"},
      "mode": "accept_edits", "model": "claude-opus-4-7", "effort": "xhigh", "contextTokens": 51234,
      "requests": [
        {"type": "permission_request", "request_id": "toolu_1", "tool_name": "Bash",
         "input": {"command": "rm -rf build", "description": "清理"}, "agent_id": null,
         "options": [{"label": "Yes", "meaning": "allow"},
                     {"label": "Yes, and don't ask again for rm commands", "meaning": "allow_always"},
                     {"label": "No", "meaning": "deny"}],
         "layout_source": "builtin"},
        {"type": "question", "request_id": "toolu_2", "agent_id": "a1b2c3d4e5f6",
         "questions": [{"question": "用哪个方案？", "header": "方案",
                        "options": [{"label": "A", "description": "快"}, {"label": "B", "preview": "x"}],
                        "multiSelect": false}]},
        {"type": "choice", "request_id": "choice:1", "title": "切换模型？",
         "options": [{"label": "Yes", "meaning": "allow", "description": null}, {"label": "No", "meaning": "deny"}],
         "tool_use_id": null, "layout": "model_switch", "layout_source": "builtin"},
        {"type": "dialog", "request_id": "dialog:trust", "waiting_for": "trust", "lines": ["", "Trust this folder?", ""]},
        {"type": "unrecognized", "request_id": "unrecognized:cost", "context": "cost", "lines": ["Total cost: $1"]},
        {"type": "rewind", "request_id": "rw1", "uuid": "u", "text": "hi", "code": null, "warning": null, "options": []},
        {"type": "teleport", "request_id": "t1"},
        {"type": "permission_request", "request_id": 7}
      ],
      "progress": {"text": "Thinking… (12s)", "verb": "Thinking", "elapsed_s": 12, "tokens": null},
      "queue": ["先这样", 3],
      "tasks": [
        {"id": "b1", "kind": "shell", "tool_use_id": "toolu_9", "description": null, "command": "npm run dev", "status": "running"},
        {"id": "m1", "kind": "monitor", "description": "watch", "status": "completed"},
        {"id": "t1", "kind": "agent", "tool_use_id": "", "description": "docs", "status": "running", "idle": true},
        {"kind": "agent"}
      ],
      "lastTurn": {"type": "turn_end", "duration_ms": 1200},
      "procExit": null, "sessionExited": false,
      "notices": [{"id": 1, "text": "分析完成"}],
      "feedbackDraft": null, "somethingNew": {"x": 1}
    }
    """

    func testDecodesEveryRequestType() throws {
        let c = try JSONDecoder().decode(ConnState.self, from: Data(Self.stateJSON.utf8))
        XCTAssertEqual(c.conn, "c1")
        XCTAssertEqual(c.state, .waiting)
        XCTAssertEqual(c.waiting?.waitingFor, "permission prompt")
        XCTAssertEqual(c.effort, "xhigh")
        XCTAssertTrue(c.canCommand)
        XCTAssertEqual(c.requests.count, 8)

        guard case .permission(let p) = c.requests[0] else { return XCTFail("permission") }
        XCTAssertEqual(p.toolName, "Bash")
        XCTAssertEqual(p.allowAlways?.label, "Yes, and don't ask again for rm commands")
        XCTAssertEqual(ConnectWords.inputBlocks(tool: p.toolName, p.input), [
            InputBlock(kind: .code, text: "rm -rf build"),
            InputBlock(kind: .note, text: "清理"),
        ])

        guard case .question(let q) = c.requests[1] else { return XCTFail("question") }
        XCTAssertEqual(q.agentId, "a1b2c3d4e5f6")
        XCTAssertEqual(q.questions.first?.header, "方案")
        XCTAssertEqual(q.questions.first?.options.map(\.label), ["A", "B"])
        XCTAssertEqual(q.questions.first?.multiSelect, false)

        guard case .choice(let ch) = c.requests[2] else { return XCTFail("choice") }
        XCTAssertEqual(ch.title, "切换模型？")
        XCTAssertEqual(ch.options.map(\.label), ["Yes", "No"])

        guard case .screen(let d) = c.requests[3] else { return XCTFail("dialog") }
        XCTAssertEqual(d.type, "dialog")
        XCTAssertEqual(d.context, "trust")
        guard case .screen(let u) = c.requests[4] else { return XCTFail("unrecognized") }
        XCTAssertEqual(u.context, "cost")
        XCTAssertEqual(u.lines, ["Total cost: $1"])

        XCTAssertEqual(c.requests[5], .other(type: "rewind", requestId: "rw1"))
        XCTAssertEqual(c.requests[6], .other(type: "teleport", requestId: "t1"))
        // A known type that does not decode is kept as a card without actions.
        guard case .other("permission_request", _) = c.requests[7] else { return XCTFail("malformed permission") }
    }

    func testKeepsWhatDecodesAndDropsTheRest() throws {
        let c = try JSONDecoder().decode(ConnState.self, from: Data(Self.stateJSON.utf8))
        XCTAssertEqual(c.queue, ["先这样"])
        XCTAssertEqual(c.tasks.map(\.id), ["b1", "m1", "t1"])
        XCTAssertEqual(c.runningTasks.map(\.name), ["npm run dev", "docs"])
        XCTAssertEqual(c.workingTasks.map(\.name), ["npm run dev"])
        XCTAssertEqual(c.progress?.elapsedS, 12)
        XCTAssertEqual(c.notices, [.init(id: 1, text: "分析完成")])
    }

    func testMinimalAndUnknownState() throws {
        let c = try JSONDecoder().decode(ConnState.self, from: Data(#"{"conn": "c9", "state": "hibernating", "ready": "yes"}"#.utf8))
        XCTAssertNil(c.state)
        XCTAssertFalse(c.ready)
        XCTAssertTrue(c.requests.isEmpty)
        XCTAssertTrue(c.isLive)
        XCTAssertFalse(c.canCommand)
        // Fields an older Host does not send.
        XCTAssertNil(c.fork)
        XCTAssertFalse(c.disconnected)
    }

    func testForkAndDisconnected() throws {
        let okJSON = #"{"conn": "c2", "disconnected": true, "fork": {"type": "fork", "session_id": "s9", "origin": "s1", "text": null, "title": "拆分 (Fork)", "title_error": "rename failed"}}"#
        let ok = try JSONDecoder().decode(ConnState.self, from: Data(okJSON.utf8))
        XCTAssertTrue(ok.disconnected)
        XCTAssertEqual(ok.fork, .init(sessionId: "s9", titleError: "rename failed"))
        let failedJSON = #"{"conn": "c3", "fork": {"type": "fork", "error": {"code": "bad_prompt", "message": "没有这个提示词"}}}"#
        let failed = try JSONDecoder().decode(ConnState.self, from: Data(failedJSON.utf8))
        XCTAssertEqual(failed.fork?.error?.message, "没有这个提示词")
        XCTAssertNil(failed.fork?.sessionId)
    }

    func testConnectEvents() throws {
        let state = try HostEvent.decode(Data(#"{"event": "connect-state-event", "payload": \#(Self.stateJSON)}"#.utf8))
        guard case .connectState(let c) = state else { return XCTFail("state") }
        XCTAssertEqual(c.sessionId, "s1")

        let removed = try HostEvent.decode(Data(#"{"event": "connect-removed-event", "payload": {"conn": "c1"}}"#.utf8))
        guard case .connectRemoved("c1") = removed else { return XCTFail("removed") }

        let ended = try HostEvent.decode(Data(#"{"event": "ccrpc-ls-ended-event", "payload": {"message": "ccrpc ls 退出了"}}"#.utf8))
        guard case .ccrpcLsEnded("ccrpc ls 退出了") = ended else { return XCTFail("ls ended") }

        let ls = try HostEvent.decode(Data(#"{"event": "ccrpc-ls-event", "payload": {"ls": "[{\"holder_id\":\"h1\",\"session_id\":\"s1\",\"plugin\":true}]"}}"#.utf8))
        guard case .ccrpcLs(let text) = ls else { return XCTFail("ls") }
        XCTAssertEqual(LsEntry.parseList(text), [LsEntry(holderId: "h1", sessionId: "s1", plugin: true)])
    }

    func testCommandResponse() throws {
        let ok = try HostClient.decodeRPC(
            Data(#"{"result": {"type": "response", "command": "prompt", "success": true, "data": {"uuid": "u1"}}}"#.utf8),
            status: 200
        ) as CommandResponse
        XCTAssertTrue(ok.success)
        XCTAssertEqual(ok.data?["uuid"], "u1")
        let refused = try HostClient.decodeRPC(
            Data(#"{"result": {"type": "response", "command": "exit", "success": false, "error": {"code": "tasks_running", "message": "Background work is running"}}}"#.utf8),
            status: 200
        ) as CommandResponse
        XCTAssertEqual(refused.error?.code, "tasks_running")
        XCTAssertEqual(refused.errorText, "Background work is running")
    }

    func testCommandEncoding() throws {
        let cmd: JSONValue = ["type": "prompt", "parts": [["text": "看图"], ["image": "/tmp/a.jpg"]], "n": 2]
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        let text = String(decoding: try encoder.encode(cmd), as: UTF8.self)
        XCTAssertEqual(text, #"{"n":2,"parts":[{"text":"看图"},{"image":"\/tmp\/a.jpg"}],"type":"prompt"}"#)
    }
}

@MainActor
final class ConnectStoreTests: XCTestCase {
    private func state(_ conn: String, session: String?, holder: String? = nil, ready: Bool = true) -> ConnState {
        var c = ConnState(conn: conn)
        c.sessionId = session
        c.holderId = holder
        c.ready = ready
        c.state = .idle
        return c
    }

    func testAppliesStatesAndRemovals() {
        let store = ConnectStore()
        store.apply(state("c1", session: "s1"))
        var busy = state("c1", session: "s1")
        busy.state = .busy
        store.apply(busy)
        XCTAssertEqual(store.conn(for: "s1")?.state, .busy)
        store.remove(conn: "c1")
        XCTAssertNil(store.conn(for: "s1"))
    }

    func testKeepsAConnectThatEndedByItselfButNotOneEndedHere() {
        let store = ConnectStore()
        var crashed = state("c1", session: "s1")
        crashed.procExit = .init(code: 1, stderr: "boom")
        store.apply(crashed)
        store.remove(conn: "c1")
        XCTAssertEqual(store.conn(for: "s1")?.procExit?.code, 1)

        var closed = state("c2", session: "s2")
        store.apply(closed)
        store.ending.insert("c2")
        closed.procExit = .init(code: 0, stderr: "")
        store.apply(closed)
        store.remove(conn: "c2")
        XCTAssertNil(store.conn(for: "s2"))
    }

    func testFindsAConnectBeforeReadyByItsHolder() {
        let store = ConnectStore()
        store.applyLs(#"[{"holder_id": "h1", "session_id": "s1", "plugin": true, "hidden": false, "extra": 1}]"#)
        XCTAssertEqual(store.lsEntry(for: "s1")?.holderId, "h1")
        store.apply(state("c1", session: nil, holder: "h1", ready: false))
        XCTAssertEqual(store.conn(for: "s1")?.conn, "c1")
        XCTAssertTrue(store.holderConnected("h1"))
        XCTAssertNil(store.conn(for: "s2"))
    }

    func testPrefersTheLiveConnectOverOneThatEnded() {
        let store = ConnectStore()
        var ended = state("c1", session: "s1")
        ended.procExit = .init(code: 1, stderr: "boom")
        store.apply(ended)
        XCTAssertEqual(store.conn(for: "s1")?.conn, "c1")
        store.apply(state("c2", session: "s1"))
        XCTAssertEqual(store.conn(for: "s1")?.conn, "c2")
        // A Session that ended (kept to say so) does not hide the new Connect either.
        var exited = state("c2", session: "s1")
        exited.sessionExited = true
        store.apply(exited)
        store.remove(conn: "c2")
        store.apply(state("c3", session: "s1"))
        XCTAssertEqual(store.conn(for: "s1")?.conn, "c3")
    }

    func testADisconnectedConnectGoesWithoutATrace() {
        let store = ConnectStore()
        var c = state("c1", session: "s1")
        c.procExit = .init(code: 0, stderr: "")
        c.disconnected = true
        store.apply(c)
        store.remove(conn: "c1")
        XCTAssertNil(store.conn(for: "s1"))
    }

    func testLsEndedClearsTheListAndSaysWhy() {
        let store = ConnectStore()
        store.applyLs(#"[{"holder_id": "h1", "session_id": "s1", "plugin": true}]"#)
        store.lsEnded("ccrpc ls 退出了")
        XCTAssertNil(store.lsEntry(for: "s1"))
        XCTAssertEqual(store.lsError, "ccrpc ls 退出了")
        store.applyLs(#"[{"holder_id": "h1", "session_id": "s1", "plugin": true}]"#)
        XCTAssertNil(store.lsError)
    }

    func testBadLsTextKeepsTheLastList() {
        let store = ConnectStore()
        store.applyLs(#"[{"holder_id": "h1", "session_id": "s1", "plugin": false}]"#)
        store.applyLs("not json")
        XCTAssertEqual(store.ls.count, 1)
        XCTAssertFalse(store.ls[0].plugin)
    }

    // MARK: Snapshots

    func testAnEventThatCameDuringASnapshotIsNewerThanIt() {
        var ended = state("c5", session: "s5")
        ended.procExit = .init(code: 0, stderr: "")
        var busy = state("c6", session: "s6")
        busy.state = .busy
        let stale = [state("c5", session: "s5"), state("c6", session: "s6"), state("c7", session: "s7")]

        let merged = ConnectStore.merged(
            local: ["c5": ended, "c6": busy], snapshot: stale, touched: ["c5", "c6", "c8"], ending: []
        )
        XCTAssertEqual(merged["c5"]?.procExit?.code, 0, "not brought back as live")
        XCTAssertEqual(merged["c6"]?.state, .busy)
        XCTAssertEqual(merged["c7"]?.sessionId, "s7", "unknown to the events: from the snapshot")
        XCTAssertNil(merged["c8"], "removed by an event while the snapshot was on its way")
    }

    func testAConnectThatEndedByItselfOutlivesASnapshot() {
        var crashed = state("c1", session: "s1")
        crashed.procExit = .init(code: 1, stderr: "boom")
        var closedHere = state("c2", session: "s2")
        closedHere.procExit = .init(code: 0, stderr: "")
        var disconnected = state("c3", session: "s3")
        disconnected.procExit = .init(code: 0, stderr: "")
        disconnected.disconnected = true
        let wentAway = state("c4", session: "s4")

        let merged = ConnectStore.merged(
            local: ["c1": crashed, "c2": closedHere, "c3": disconnected, "c4": wentAway],
            snapshot: [], touched: [], ending: ["c2"]
        )
        XCTAssertEqual(Array(merged.keys), ["c1"])
    }

    // MARK: A page following its Connect

    func testAnOpenPageFollowsItsConnectToTheNewSession() {
        let store = ConnectStore()
        store.pageOpened("s1")
        store.apply(state("c1", session: "s1"))
        store.apply(state("c1", session: "s2"))
        XCTAssertEqual(store.moved, ["s1": "s2"])
        // Until the page takes it: the bar (and what is typed in it) stays.
        XCTAssertEqual(store.conn(for: "s1")?.conn, "c1")
    }

    func testAPageNotOnScreenDoesNotFollow() {
        let store = ConnectStore()
        store.pageOpened("s1")
        store.pageClosed("s1")
        store.apply(state("c1", session: "s1"))
        store.apply(state("c1", session: "s2"))
        XCTAssertTrue(store.moved.isEmpty)
        XCTAssertNil(store.conn(for: "s1"))
    }

    func testAForkSettingUpIsNotAMove() {
        let store = ConnectStore()
        store.pageOpened("origin")
        store.starts["c1"] = .fork(origin: "origin")
        store.apply(state("c1", session: "origin"))
        store.apply(state("c1", session: "forked"))
        XCTAssertTrue(store.moved.isEmpty)
    }

    func testFirstNameOfANewSessionIsNotAMove() {
        let store = ConnectStore()
        store.pageOpened("s1")
        store.apply(state("c1", session: nil))
        store.apply(state("c1", session: "s1"))
        XCTAssertTrue(store.moved.isEmpty)
    }

    func testAResumeBeforeItsNameIsFoundOnItsSession() {
        let store = ConnectStore()
        store.starts["c1"] = .resume(sessionId: "s1")
        store.apply(state("c1", session: nil, ready: false))
        XCTAssertEqual(store.conn(for: "s1")?.conn, "c1")
        XCTAssertNil(store.conn(for: "s2"))
    }

    func testListsMainLineRequestsFirst() throws {
        let store = ConnectStore()
        let c = try JSONDecoder().decode(ConnState.self, from: Data(ConnStateDecodingTests.stateJSON.utf8))
        store.apply(c)
        let ids = store.requests(of: c).map(\.id)
        XCTAssertEqual(ids.last, "toolu_2")
        XCTAssertEqual(ids.first, "toolu_1")
    }
}

/// Mirrors src/features/connect/answers.test.ts.
final class AnswersTests: XCTestCase {
    private func q(_ question: String, multi: Bool = false) -> AskQuestion {
        AskQuestion(question: question, multiSelect: multi)
    }

    func testPermissionLabel() {
        let opts = { (allow: String, deny: String) in
            [PermissionRequest.Option(label: allow, meaning: "allow"), .init(label: deny, meaning: "deny")]
        }
        let builtin = PermissionRequest(requestId: "r", toolName: "Bash", input: nil, options: opts("Yes", "No"), layoutSource: "builtin")
        XCTAssertEqual(Answers.permissionLabel(builtin, meaning: "allow", fallback: "允许"), "允许")
        XCTAssertEqual(Answers.permissionLabel(builtin, meaning: "deny", fallback: "拒绝"), "拒绝")
        let learned = PermissionRequest(requestId: "r", toolName: "Bash", input: nil, options: opts("Yes", "No"), layoutSource: "learned")
        XCTAssertEqual(Answers.permissionLabel(learned, meaning: "allow", fallback: "允许"), "Yes")
        let none = PermissionRequest(requestId: "r", toolName: "Bash", input: nil)
        XCTAssertEqual(Answers.permissionLabel(none, meaning: "allow", fallback: "允许"), "允许")
        let grant = PermissionRequest(
            requestId: "r", toolName: "computer", input: nil,
            options: opts("Allow for this session (4 apps)", "Deny, and tell Claude what to do differently (esc)"),
            layoutSource: "builtin"
        )
        XCTAssertEqual(Answers.permissionLabel(grant, meaning: "allow", fallback: "允许"), "Allow for this session (4 apps)")
        XCTAssertEqual(Answers.permissionLabel(grant, meaning: "deny", fallback: "拒绝"), "拒绝")
        let workflow = PermissionRequest(requestId: "r", toolName: "W", input: nil, options: opts("Yes, run it", "No"), layoutSource: "builtin")
        XCTAssertEqual(Answers.permissionLabel(workflow, meaning: "allow", fallback: "允许"), "允许")
    }

    func testTogglePick() {
        XCTAssertEqual(Answers.togglePick([], "A", multi: false), ["A"])
        XCTAssertEqual(Answers.togglePick(["A"], "B", multi: false), ["B"])
        XCTAssertEqual(Answers.togglePick(["A"], "A", multi: false), [])
        XCTAssertEqual(Answers.togglePick(["A"], "B", multi: true), ["A", "B"])
        XCTAssertEqual(Answers.togglePick(["A", "B", "C"], "B", multi: true), ["A", "C"])
    }

    func testWrittenAnswerWhereOtherIsChosenAndNotBlank() {
        let answers = Answers.questionAnswers(
            [q("a"), q("b"), q("c")],
            picked: [["X"], [], ["Y"]], other: ["", "  写的  ", "   "], useOther: [false, true, true]
        )
        XCTAssertEqual(answers, [["options": ["X"]], ["other": "写的"], ["options": ["Y"]]])
    }

    func testMultiSelectSendsOptionsAndWrittenTogether() {
        let answers = Answers.questionAnswers(
            [q("a", multi: true), q("b", multi: true), q("c", multi: true)],
            picked: [["X", "Y"], [], ["Z"]], other: [" 写的 ", "只有字", "划掉的"], useOther: [true, true, false]
        )
        XCTAssertEqual(answers, [["options": ["X", "Y"], "other": "写的"], ["other": "只有字"], ["options": ["Z"]]])
    }

    func testTextSetAsideDoesNotCount() {
        XCTAssertFalse(Answers.otherChosen(["写的"], [false], 0))
        XCTAssertTrue(Answers.otherChosen(["写的"], [true], 0))
    }

    func testAnswerError() {
        XCTAssertEqual(Answers.error(.failed("x", code: "not_offered")), "这次审批没有“总是允许”选项")
        XCTAssertEqual(Answers.error(.failed("按键超时", code: "timeout")), "作答失败：按键超时")
        XCTAssertEqual(Answers.error(.failed("", code: "timeout")), "作答失败：timeout")
    }

    func testModelLabel() {
        XCTAssertEqual(ConnectWords.modelLabel("claude-opus-4-7"), "Opus 4.7")
        XCTAssertEqual(ConnectWords.modelLabel("claude-fable-5-1[1m]"), "Fable 5.1 (1M)")
        XCTAssertEqual(ConnectWords.modelLabel("claude-sonnet-4-20250514"), "Sonnet 4")
        XCTAssertEqual(ConnectWords.modelLabel("Sonnet 4.5"), "Sonnet 4.5")
        XCTAssertEqual(ConnectWords.modelFamily("claude-opus-4-7"), "opus")
    }
}

/// Mirrors src/features/connect/slash.test.ts.
final class SlashRulesTests: XCTestCase {
    private static func cmd(_ name: String, _ source: String = "builtin", interactive: Bool = false) -> SlashCommand {
        SlashCommand(name: name, source: source, interactive: interactive)
    }

    private let list = [
        cmd("clear"), cmd("compact"), cmd("cost"), cmd("release-notes"), cmd("release", "project"),
        cmd("kit:review", "plugin"), cmd("review"), cmd("omc:autopilot", "plugin"), cmd("omc:plan", "plugin"),
        cmd("kit:plan", "plugin"), cmd("dir:test", "project"), cmd("model", interactive: true),
    ]

    func testCommandOf() {
        XCTAssertEqual(SlashRules.commandOf("/review the PR", list)?.name, "review")
        XCTAssertEqual(SlashRules.commandOf("/kit:review", list)?.name, "kit:review")
        XCTAssertEqual(SlashRules.commandOf("/autopilot build it", list)?.name, "omc:autopilot")
        XCTAssertNil(SlashRules.commandOf("/plan", list), "two plugins have it")
        XCTAssertNil(SlashRules.commandOf("/kit:autopilot", list))
        XCTAssertNil(SlashRules.commandOf("/auto", list))
        XCTAssertNil(SlashRules.commandOf("/test", list), "only a plugin's goes by its short name")
        XCTAssertNil(SlashRules.commandOf("autopilot", list))
    }

    func testBuiltInAndUnknownCommandsGoAsSlash() {
        XCTAssertEqual(SlashRules.slashCommand("/compact keep the API notes", withImages: false, list), "compact keep the API notes")
        XCTAssertEqual(SlashRules.slashCommand("/doctor", withImages: false, list), "doctor")
    }

    func testGoingBackToAPromptIsRefusedHere() {
        XCTAssertEqual(SlashRules.refusedHere("/rewind"), "iPhone 上不能回退")
        XCTAssertEqual(SlashRules.refusedHere("/checkpoint now"), "iPhone 上不能回退")
        XCTAssertNil(SlashRules.refusedHere("/rewinder"))
        XCTAssertNil(SlashRules.refusedHere("rewind"))
    }

    func testTheRestGoesAsAPrompt() {
        XCTAssertNil(SlashRules.slashCommand("/cost", withImages: false, nil), "list unknown")
        XCTAssertNil(SlashRules.slashCommand("/Users/me/shot.png 这是什么", withImages: false, list))
        XCTAssertNil(SlashRules.slashCommand("/release v1", withImages: false, list))
        XCTAssertNil(SlashRules.slashCommand("/kit:review", withImages: false, list))
        XCTAssertNil(SlashRules.slashCommand("/autopilot go", withImages: false, list))
        XCTAssertNil(SlashRules.slashCommand("/compact\nmore", withImages: false, list))
        XCTAssertNil(SlashRules.slashCommand("/cost", withImages: true, list))
        XCTAssertNil(SlashRules.slashCommand("/", withImages: false, list))
        XCTAssertNil(SlashRules.slashCommand("hello", withImages: false, list))
    }

    func testAPanelIsRefusedWhileBusy() {
        XCTAssertEqual(SlashRules.refusedWhileBusy("/model opus", list), "/model 会在终端里打开界面，等空闲时再发")
        XCTAssertNil(SlashRules.refusedWhileBusy("/compact", list))
        XCTAssertNil(SlashRules.refusedWhileBusy("/model", nil))
        XCTAssertNil(SlashRules.refusedWhileBusy("继续", list))
    }
}

/// A permission card shows all of a tool's input.
final class InputBlocksTests: XCTestCase {
    func testEditShowsBothSides() {
        let blocks = ConnectWords.inputBlocks(tool: "Edit", [
            "file_path": "/w/a.swift", "old_string": "let a = 1", "new_string": "let a = 2", "replace_all": true,
        ])
        XCTAssertEqual(blocks, [
            InputBlock(kind: .path, text: "/w/a.swift"),
            InputBlock(kind: .removed, text: "let a = 1", label: "改动前"),
            InputBlock(kind: .added, text: "let a = 2", label: "改动后"),
            InputBlock(kind: .code, text: "{\n  \"replace_all\" : true\n}"),
        ])
    }

    func testWriteAndCommandAreNotCut() {
        let long = (1...400).map { "line \($0)" }.joined(separator: "\n")
        XCTAssertEqual(ConnectWords.inputBlocks(tool: "Write", ["file_path": "/w/a", "content": .string(long)]), [
            InputBlock(kind: .path, text: "/w/a"),
            InputBlock(kind: .added, text: long, label: "写入内容"),
        ])
        XCTAssertEqual(ConnectWords.inputBlocks(tool: "Bash", ["command": .string(long)]), [InputBlock(kind: .code, text: long)])
    }

    func testNotebookEditShowsItsSource() {
        let blocks = ConnectWords.inputBlocks(tool: "NotebookEdit", ["notebook_path": "/w/n.ipynb", "new_source": "print(1)"])
        XCTAssertEqual(blocks.map(\.text), ["/w/n.ipynb", "print(1)"])
    }

    func testAnyOtherToolShowsEveryField() {
        let blocks = ConnectWords.inputBlocks(tool: "mcp__x__do", ["content": "正文", "deep": ["a": [1, 2]]])
        XCTAssertEqual(blocks.count, 1)
        XCTAssertTrue(blocks[0].text.contains("正文"))
        XCTAssertTrue(blocks[0].text.contains("\"deep\""))
        XCTAssertEqual(blocks[0].kind, .code)
    }

    func testNothingToShow() {
        XCTAssertEqual(ConnectWords.inputBlocks(tool: "Bash", nil), [])
        XCTAssertEqual(ConnectWords.inputBlocks(tool: "Bash", .null), [])
        XCTAssertEqual(ConnectWords.inputBlocks(tool: "Bash", [:]), [])
    }
}

@MainActor
final class TrailingThrottleTests: XCTestCase {
    func testABurstRunsOnceAfterTheIntervalAndALaterChangeRunsAgain() async throws {
        var runs = 0
        let throttle = TrailingThrottle(interval: .milliseconds(40)) { runs += 1 }
        for _ in 0..<5 { throttle.fire() }
        XCTAssertEqual(runs, 0, "trailing, not leading")
        try await Task.sleep(for: .milliseconds(120))
        XCTAssertEqual(runs, 1)
        throttle.fire()
        try await Task.sleep(for: .milliseconds(120))
        XCTAssertEqual(runs, 2, "a change after the run is not lost")
    }

    func testCancelDropsWhatIsPending() async throws {
        var runs = 0
        let throttle = TrailingThrottle(interval: .milliseconds(20)) { runs += 1 }
        throttle.fire()
        throttle.cancel()
        try await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(runs, 0)
    }
}

/// 「关闭 Session」 (Header.tsx ConnectClose, docs/spec.md Connect).
final class CloseFlowTests: XCTestCase {
    func testConfirmationSaysWhatEnds() {
        let flow = CloseFlow()
        XCTAssertEqual(flow.message(busy: false, running: 0), "结束 Session 并关闭它的 Pane。")
        XCTAssertEqual(flow.message(busy: true, running: 2), "将中止本轮、停掉 2 个后台任务，结束 Session 并关闭它的 Pane。")
        XCTAssertFalse(flow.stopTasks(running: 0))
        XCTAssertTrue(flow.stopTasks(running: 1))
    }

    func testTasksRunningAsksAgainThenStopsThem() {
        var flow = CloseFlow()
        let refused = CommandResponse(
            command: "exit", success: false, data: nil,
            error: .init(code: "tasks_running", message: "Background work is running")
        )
        XCTAssertEqual(flow.outcome(of: refused), .confirmTasks)
        XCTAssertEqual(flow.message(busy: false, running: 0), "将停掉后台任务，结束 Session 并关闭它的 Pane。")
        XCTAssertTrue(flow.stopTasks(running: 0))
        XCTAssertEqual(flow.outcome(of: CommandResponse(command: "exit", success: true)), .done)
    }

    func testOtherFailuresAreShown() {
        var flow = CloseFlow()
        XCTAssertEqual(flow.outcome(of: .failed("ccrpc 已退出", code: "exited")), .failed("关闭失败：ccrpc 已退出"))
        XCTAssertFalse(flow.tasksRan)
    }
}
