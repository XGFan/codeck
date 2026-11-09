import Foundation

// Words and pure rules shared with the desktop's Connect (src/features/connect: Header.tsx, Model.tsx,
// answers.ts, Tasks.tsx); kept identical in meaning.

enum ConnectWords {
    /// The one word for a connection's state (Header.tsx `STATE_WORD`).
    static func state(_ phase: ConnState.Phase?) -> String {
        switch phase {
        case .starting: "启动中"
        case .idle: "空闲"
        case .busy: "工作中"
        case .waiting: "等待作答"
        case .exited: "已结束"
        case nil: "启动中"
        }
    }

    /// Permission modes for `set_mode` (Header.tsx `MODES`).
    static let modes: [(id: String, label: String)] = [
        ("manual", "手动审批"),
        ("accept_edits", "自动接受编辑"),
        ("plan", "计划模式"),
        ("auto", "自动模式"),
    ]

    /// The aliases `set_model` takes (Model.tsx `MODELS`).
    static let models: [(alias: String, label: String)] = [
        ("default", "默认"),
        ("opus", "Opus"),
        ("fable", "Fable"),
        ("sonnet", "Sonnet"),
        ("haiku", "Haiku"),
    ]

    /// Effort levels for `set_effort`; xhigh is 「Extra」 (Model.tsx `EFFORTS`).
    static let efforts: [(level: String, label: String)] = [
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
        ("xhigh", "Extra"),
        ("max", "Max"),
    ]

    static func modeLabel(_ mode: String?) -> String {
        guard let mode else { return "权限模式" }
        return modes.first { $0.id == mode }?.label ?? mode
    }

    static func effortLabel(_ effort: String) -> String {
        efforts.first { $0.level == effort }?.label ?? effort.prefix(1).uppercased() + effort.dropFirst()
    }

    /// `claude-opus-4-7` → `Opus 4.7`, `claude-fable-5-1[1m]` → `Fable 5.1 (1M)`; anything else as it is.
    static func modelLabel(_ model: String) -> String {
        let pattern = #"^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?(\[1m\])?$"#
        let s = model.trimmingCharacters(in: .whitespaces)
        guard let re = try? NSRegularExpression(pattern: pattern, options: .caseInsensitive),
              let m = re.firstMatch(in: s, range: NSRange(s.startIndex..., in: s))
        else { return model }
        func group(_ i: Int) -> String? { Range(m.range(at: i), in: s).map { String(s[$0]) } }
        let family = group(1)!.prefix(1).uppercased() + group(1)!.dropFirst().lowercased()
        let minor = group(3).map { ".\($0)" } ?? ""
        return "\(family) \(group(2)!)\(minor)\(group(4) != nil ? " (1M)" : "")"
    }

    /// The alias family a model belongs to, for the check mark.
    static func modelFamily(_ model: String?) -> String? {
        guard let model = model?.lowercased() else { return nil }
        return ["opus", "fable", "sonnet", "haiku"].first { model.contains($0) }
    }

    /// Task kinds in list order (Tasks.tsx `KINDS`).
    static func taskKind(_ kind: String) -> (label: String, symbol: String) {
        switch kind {
        case "shell": ("Shell", "apple.terminal")
        case "agent": ("Subagent", "point.3.connected.trianglepath.dotted")
        case "monitor": ("Monitor", "dot.radiowaves.left.and.right")
        case "workflow": ("Workflow", "flowchart")
        default: (kind, "apple.terminal")
        }
    }

    static let taskKindOrder = ["shell", "agent", "monitor", "workflow"]

    static func taskStatus(_ status: String) -> String {
        switch status {
        case "running": "运行中"
        case "completed": "已完成"
        case "failed", "error": "失败"
        case "killed": "已终止"
        case "stopped": "已停止"
        default: status
        }
    }

    /// Seconds as a process detail (docs/spec.md): 「12 秒」, 「1 分 04 秒」, 「1 小时 02 分」.
    static func elapsed(_ seconds: Double) -> String {
        let s = max(0, Int(seconds))
        if s < 60 { return "\(s) 秒" }
        if s < 3600 { return "\(s / 60) 分 \(String(format: "%02d", s % 60)) 秒" }
        return "\(s / 3600) 小时 \(String(format: "%02d", (s % 3600) / 60)) 分"
    }

    /// Why ccrpc's process ended.
    static func procExit(_ e: ConnState.ProcExit) -> String {
        "ccrpc 已退出（\(e.code.map { "退出码 \($0)" } ?? "被信号终止")）"
    }

    /// A tool input in short: its command or path when it has one, else the first lines of its JSON.
    static func inputSummary(_ input: JSONValue?) -> String {
        guard let input else { return "" }
        for key in ["command", "file_path", "notebook_path", "url", "pattern", "path"] {
            if let s = input[key]?.stringValue, !s.isEmpty { return s }
        }
        if case .object(let o) = input, o.isEmpty { return "" }
        return input.pretty.split(separator: "\n", omittingEmptySubsequences: false).prefix(8).joined(separator: "\n")
    }

    /// The last non-blank line of ccrpc's stderr.
    static func lastLine(_ s: String?) -> String? {
        s?.split(whereSeparator: \.isNewline).last { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }.map(String.init)
    }
}

// MARK: Answers (answers.ts)

enum Answers {
    /// What to say when ccrpc refuses an answer.
    static func error(_ r: CommandResponse) -> String {
        r.error?.code == "not_offered" ? "这次审批没有“总是允许”选项" : "作答失败：\(r.errorText)"
    }

    /// The words on a permission button for `meaning`: the dialog's own for a learned layout, and for an allow
    /// that says more than a plain `Yes…`; otherwise `fallback`.
    static func permissionLabel(_ r: PermissionRequest, meaning: String, fallback: String) -> String {
        guard let own = r.options?.first(where: { $0.meaning == meaning })?.label else { return fallback }
        if r.layoutSource == "learned" { return own }
        let yes = own.range(of: #"^yes\b"#, options: [.regularExpression, .caseInsensitive]) != nil
        return meaning == "allow" && !yes ? own : fallback
    }

    /// Picking an option: toggles it among the others when several can be picked, otherwise it is the only one.
    static func togglePick(_ picked: [String], _ label: String, multi: Bool) -> [String] {
        let on = picked.contains(label)
        if multi { return on ? picked.filter { $0 != label } : picked + [label] }
        return on ? [] : [label]
    }

    /// Question `i` is answered by its written text: 其他 is chosen and something is written.
    static func otherChosen(_ other: [String], _ useOther: [Bool], _ i: Int) -> Bool {
        i < useOther.count && useOther[i] && i < other.count && !other[i].trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// The `answers` of `answer_question`, one per question: on a single-select question the written answer
    /// when 其他 is chosen and not blank, otherwise the option picked; on a multi-select one the options picked
    /// and the written answer, either or both.
    static func questionAnswers(_ qs: [AskQuestion], picked: [[String]], other: [String], useOther: [Bool]) -> [JSONValue] {
        qs.indices.map { i in
            let options = i < picked.count ? picked[i] : []
            let optionsValue = JSONValue.array(options.map(JSONValue.string))
            guard otherChosen(other, useOther, i) else { return ["options": optionsValue] }
            let written = JSONValue.string(other[i].trimmingCharacters(in: .whitespacesAndNewlines))
            return qs[i].multiSelect && !options.isEmpty ? ["options": optionsValue, "other": written] : ["other": written]
        }
    }
}

// MARK: Closing (Header.tsx ConnectClose)

/// 「关闭 Session」: confirm, then `exit {close_pane}` (an `abort` first while busy); when Claude Code says
/// background work still runs (`tasks_running`), confirm again and resend with `stop_tasks`.
struct CloseFlow: Equatable {
    /// Claude Code said on exit that background work still runs.
    var tasksRan = false

    /// The confirmation's sentence.
    func message(busy: Bool, running: Int) -> String {
        var ends: [String] = []
        if busy { ends.append("中止本轮") }
        if running > 0 { ends.append("停掉 \(running) 个后台任务") } else if tasksRan { ends.append("停掉后台任务") }
        return ends.isEmpty ? "结束 Session 并关闭它的 Pane。" : "将\(ends.joined(separator: "、"))，结束 Session 并关闭它的 Pane。"
    }

    func stopTasks(running: Int) -> Bool { running > 0 || tasksRan }

    enum Outcome: Equatable {
        case done
        /// Confirm again: the exit was refused because background work runs.
        case confirmTasks
        case failed(String)
    }

    mutating func outcome(of r: CommandResponse) -> Outcome {
        if r.success { return .done }
        if r.error?.code == "tasks_running" {
            tasksRan = true
            return .confirmTasks
        }
        return .failed("关闭失败：\(r.errorText)")
    }
}
