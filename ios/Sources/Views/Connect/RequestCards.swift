import SwiftUI

/// One thing the Session waits on (Requests.tsx).
struct RequestCard: View {
    let request: ConnRequest
    let c: ConnState

    var body: some View {
        switch request {
        case .permission(let r): PermissionCard(r: r, c: c)
        case .question(let r): QuestionCard(r: r, c: c).id("\(r.requestId)|\(r.questions.count)")
        case .choice(let r): ChoiceCard(r: r, c: c)
        case .screen(let r):
            CardShell(
                icon: "rectangle.dashed",
                title: r.type == "dialog" ? "Claude Code 停在一个 ccrpc 认不出的界面" : "认不出的屏幕（\(r.context ?? "")）",
                status: "等待处理"
            ) {
                ScreenLines(lines: r.lines)
                Text("请在终端里处理").font(.footnote).foregroundStyle(.secondary)
            }
        case .other(let type, _):
            CardShell(icon: "hourglass", title: type == "rewind" ? "回退确认" : "Claude Code 在等待操作", status: "等待处理") {
                Text("这里还不能作答，请在终端里处理").font(.footnote).foregroundStyle(.secondary)
            }
        }
    }
}

/// A card's frame: an icon, a title, what it waits for, then the body.
struct CardShell<Content: View>: View {
    let icon: String
    let title: String
    let status: String
    var agent: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: icon).foregroundStyle(.tint)
                Text(title).font(.subheadline.weight(.semibold)).lineLimit(2)
                if let agent {
                    Text("Subagent \(agent.prefix(8))")
                        .font(.caption2)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Color(uiColor: .tertiarySystemFill), in: RoundedRectangle(cornerRadius: 4))
                }
                Spacer(minLength: 4)
                Text(status).font(.caption).foregroundStyle(.orange)
            }
            content
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(.orange.opacity(0.5)))
    }
}

/// Sends an answer; its error stays on the card.
@MainActor
private func sendAnswer(
    _ store: ConnectStore, _ c: ConnState, _ requestId: String, _ command: JSONValue,
    busy: Binding<Bool>, error: Binding<String?>, answered: Binding<Int>
) async {
    busy.wrappedValue = true
    error.wrappedValue = nil
    let r = await store.answer(c, requestId: requestId, command)
    busy.wrappedValue = false
    if r.success { answered.wrappedValue += 1 } else { error.wrappedValue = Answers.error(r) }
}

private struct PermissionCard: View {
    let r: PermissionRequest
    let c: ConnState
    @Environment(HostModel.self) private var model
    @State private var feedback = ""
    @State private var busy = false
    @State private var error: String?
    @State private var answered = 0

    var body: some View {
        CardShell(icon: "hand.raised", title: "允许 \(r.toolName)？", status: "等待审批", agent: r.agentId) {
            let summary = ConnectWords.inputSummary(r.input)
            if !summary.isEmpty {
                Text(summary)
                    .font(.footnote.monospaced())
                    .lineLimit(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(8)
                    .background(Color(uiColor: .tertiarySystemFill), in: RoundedRectangle(cornerRadius: 8))
            }
            VStack(spacing: 8) {
                answerButton(Answers.permissionLabel(r, meaning: "allow", fallback: "允许"), prominent: true) {
                    answer("allow")
                }
                if let always = r.allowAlways {
                    answerButton(always.label) { answer("allow_always") }
                }
                answerButton(Answers.permissionLabel(r, meaning: "deny", fallback: "拒绝")) { answer("deny") }
                HStack(spacing: 8) {
                    TextField("拒绝，并告诉 Claude", text: $feedback, axis: .vertical)
                        .lineLimit(1...4)
                        .submitLabel(.send)
                        .onSubmit(denyWithMessage)
                    if !feedback.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        Button("发送", systemImage: "arrow.up.circle.fill", action: denyWithMessage)
                            .labelStyle(.iconOnly)
                            .font(.title2)
                            .disabled(busy)
                    }
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(Color(uiColor: .tertiarySystemFill), in: RoundedRectangle(cornerRadius: 10))
            }
            if let error { Text(error).font(.footnote).foregroundStyle(.red) }
        }
        .disabled(busy)
        .sensoryFeedback(.success, trigger: answered)
    }

    private func answerButton(_ title: String, prominent: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).frame(maxWidth: .infinity)
        }
        .buttonStyle(ProminenceStyle(prominent: prominent))
    }

    private func answer(_ decision: String, message: String? = nil) {
        var cmd: [String: JSONValue] = ["type": "answer_permission", "request_id": .string(r.requestId), "decision": .string(decision)]
        if let message { cmd["message"] = .string(message) }
        Task {
            await sendAnswer(model.connect, c, r.requestId, .object(cmd), busy: $busy, error: $error, answered: $answered)
        }
    }

    private func denyWithMessage() {
        let text = feedback.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !busy else { return }
        answer("deny", message: text)
    }
}

private struct ChoiceCard: View {
    let r: ChoiceRequest
    let c: ConnState
    @Environment(HostModel.self) private var model
    @State private var busy = false
    @State private var error: String?
    @State private var answered = 0

    var body: some View {
        CardShell(icon: "checklist", title: r.title, status: "等待选择") {
            VStack(spacing: 8) {
                ForEach(Array(r.options.enumerated()), id: \.offset) { i, o in
                    Button {
                        let cmd: JSONValue = ["type": "answer_choice", "request_id": .string(r.requestId), "option": .string(o.label)]
                        Task {
                            await sendAnswer(model.connect, c, r.requestId, cmd, busy: $busy, error: $error, answered: $answered)
                        }
                    } label: {
                        VStack(spacing: 2) {
                            Text(o.label)
                            if let d = o.description, !d.isEmpty {
                                Text(d).font(.caption).opacity(0.75)
                            }
                        }
                        .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(ProminenceStyle(prominent: i == 0))
                }
            }
            if let error { Text(error).font(.footnote).foregroundStyle(.red) }
        }
        .disabled(busy)
        .sensoryFeedback(.success, trigger: answered)
    }
}

/// The lines of a screen ccrpc cannot read, as the terminal shows them.
private struct ScreenLines: View {
    let lines: [String]

    var body: some View {
        ScrollView(.horizontal) {
            Text(trimmed.joined(separator: "\n"))
                .font(.caption.monospaced())
                .foregroundStyle(Color(white: 0.9))
                .fixedSize()
                .padding(8)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(white: 0.12), in: RoundedRectangle(cornerRadius: 8))
    }

    private var trimmed: [String] {
        let blank = { (s: String) in s.trimmingCharacters(in: .whitespaces).isEmpty }
        guard let a = lines.firstIndex(where: { !blank($0) }), let b = lines.lastIndex(where: { !blank($0) }) else { return [] }
        return Array(lines[a...b])
    }
}

/// A full-width answer: filled for the default one, tinted otherwise.
struct ProminenceStyle: ButtonStyle {
    var prominent: Bool
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.body.weight(prominent ? .semibold : .regular))
            .padding(.vertical, 10)
            .padding(.horizontal, 12)
            .foregroundStyle(prominent ? AnyShapeStyle(.white) : AnyShapeStyle(.tint))
            .background(
                prominent ? AnyShapeStyle(.tint) : AnyShapeStyle(Color.accentColor.opacity(0.12)),
                in: RoundedRectangle(cornerRadius: 10)
            )
            .opacity(configuration.isPressed ? 0.6 : isEnabled ? 1 : 0.5)
    }
}
