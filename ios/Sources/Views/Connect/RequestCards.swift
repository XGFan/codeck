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
        case .screen(let r): ScreenCard(r: r, c: c)
        case .other(let type, _): OtherRequestCard(type: type, c: c)
        }
    }
}

/// A request the phone cannot answer: the input box is not there meanwhile, so the turn can still be stopped here.
private struct OtherRequestCard: View {
    let type: String
    let c: ConnState
    @Environment(HostModel.self) private var model
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        CardShell(title: type == "rewind" ? "回退确认" : "Claude Code 在等待操作", status: "等待处理") {
            Text("这里还不能作答，请在终端里处理").font(.footnote).foregroundStyle(.secondary)
            Button("中止") {
                Task {
                    busy = true
                    error = nil
                    let r = await model.connect.command(c.conn, ["type": "abort"])
                    busy = false
                    if !r.success { error = "中止失败：\(r.errorText)" }
                }
            }
            .buttonStyle(PillButtonStyle(kind: .neutral, height: 44))
            .disabled(busy || !c.isBusy)
            if let error { Text(error).font(.footnote).foregroundStyle(Palette.error) }
        }
    }
}

/// A request's title, what it waits for, then its body. The panel is its background.
struct CardShell<Content: View>: View {
    let title: String
    let status: String
    var agent: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 8) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(.headline)
                    if let agent {
                        Text("Subagent \(agent.prefix(8))")
                            .font(.caption2)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(Palette.neutral, in: RoundedRectangle(cornerRadius: 4))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Text(status)
                    .font(.footnote)
                    .foregroundStyle(Palette.warnText)
                    .padding(.top, 2)
            }
            .padding(.horizontal, 4)
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Sends an answer; its error stays on the card.
@MainActor
func sendAnswer(
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
        CardShell(title: "允许 \(r.toolName)？", status: "等待审批", agent: r.agentId) {
            InputBlocksView(blocks: ConnectWords.inputBlocks(tool: r.toolName, r.input))
            Button(Answers.permissionLabel(r, meaning: "allow", fallback: "允许")) { answer("allow") }
                .buttonStyle(PillButtonStyle())
            if let always = r.allowAlways {
                Button(always.label) { answer("allow_always") }
                    .buttonStyle(PillButtonStyle(kind: .tinted, height: 48))
            }
            Button(Answers.permissionLabel(r, meaning: "deny", fallback: "拒绝")) { answer("deny") }
                .buttonStyle(PillButtonStyle(kind: .neutral, height: 48))
            HStack(spacing: 8) {
                TextField("拒绝，并告诉 Claude", text: $feedback, axis: .vertical)
                    .font(.system(size: 15))
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
            .padding(.horizontal, 14)
            .padding(.vertical, 6)
            .frame(minHeight: 44)
            .background(Palette.field, in: RoundedRectangle(cornerRadius: 22, style: .continuous))
            if let error { Text(error).font(.footnote).foregroundStyle(Palette.error) }
        }
        .disabled(busy)
        .sensoryFeedback(.success, trigger: answered)
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
        CardShell(title: r.title, status: "等待选择") {
            ForEach(Array(r.options.enumerated()), id: \.offset) { i, o in
                Button {
                    let cmd: JSONValue = ["type": "answer_choice", "request_id": .string(r.requestId), "option": .string(o.label)]
                    Task {
                        await sendAnswer(model.connect, c, r.requestId, cmd, busy: $busy, error: $error, answered: $answered)
                    }
                } label: {
                    VStack(spacing: 1) {
                        Text(o.label)
                        if let d = o.description, !d.isEmpty {
                            Text(d).font(.caption).opacity(0.8)
                        }
                    }
                    .padding(.vertical, 8)
                }
                .buttonStyle(PillButtonStyle(kind: i == 0 ? .primary : .tinted))
            }
            if let error { Text(error).font(.footnote).foregroundStyle(Palette.error) }
        }
        .disabled(busy)
        .sensoryFeedback(.success, trigger: answered)
    }
}

/// All of a tool's input in one scroller inside the card, so the answers stay within reach however long it is.
struct InputBlocksView: View {
    let blocks: [InputBlock]
    var maxHeight: CGFloat = 160
    /// Whether some of the input is out of view.
    var onOverflow: (Bool) -> Void = { _ in }
    @State private var height: CGFloat = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                        InputBlockView(block: block)
                    }
                }
                .onGeometryChange(for: CGFloat.self, of: \.size.height) { height = $0 }
            }
            .frame(height: min(height, maxHeight))
            .scrollIndicators(height > maxHeight ? .visible : .hidden)
            .scrollBounceBehavior(.basedOnSize)
            if height > maxHeight {
                Text("内容较长，上下滑动查看全部").font(.caption).foregroundStyle(.secondary).padding(.horizontal, 4)
            }
        }
        .onChange(of: height > maxHeight, initial: true) { _, over in onOverflow(over) }
    }
}

private struct InputBlockView: View {
    let block: InputBlock

    var body: some View {
        switch block.kind {
        case .note:
            Text(block.text).font(.footnote).foregroundStyle(.secondary).padding(.horizontal, 4)
        case .path:
            Text(block.text).font(.caption.monospaced()).foregroundStyle(.secondary).padding(.horizontal, 4)
        case .code, .removed, .added:
            VStack(alignment: .leading, spacing: 3) {
                if let label = block.label {
                    Text(label).font(.caption).foregroundStyle(.secondary).padding(.horizontal, 4)
                }
                Text(block.text)
                    .font(.footnote.monospaced())
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .background(tint, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            }
        }
    }

    private var tint: Color {
        switch block.kind {
        case .removed: Color(light: 0xFCEAE9, dark: 0x3A2220)
        case .added: Color(light: 0xE7F5EC, dark: 0x17301F)
        default: Palette.field
        }
    }
}

/// A screen ccrpc cannot read (a panel such as `/cost`'s, an unknown dialog): shown as is; Esc closes it (`abort`
/// while busy or waiting; otherwise ccrpc refuses that, so the key itself).
private struct ScreenCard: View {
    let r: ScreenRequest
    let c: ConnState
    @Environment(HostModel.self) private var model
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        CardShell(
            title: r.type == "dialog" ? "Claude Code 停在一个 ccrpc 认不出的界面" : "认不出的屏幕（\(r.context ?? "")）",
            status: "等待处理"
        ) {
            ScreenLines(lines: r.lines)
            HStack(spacing: 12) {
                Button { Task { await close() } } label: {
                    Text("关闭（Esc）").font(.system(size: 15))
                }
                .buttonStyle(PillButtonStyle(kind: .neutral, height: 44))
                .fixedSize()
                .disabled(busy)
                Text("其他操作请在终端里处理").font(.footnote).foregroundStyle(.secondary)
            }
            if let error { Text(error).font(.footnote).foregroundStyle(Palette.error) }
        }
    }

    private func close() async {
        busy = true
        error = nil
        let esc: JSONValue = c.isBusy ? ["type": "abort"] : ["type": "send_keys", "keys": ["esc"]]
        let r = await model.connect.command(c.conn, esc)
        busy = false
        if !r.success { error = "关闭失败：\(r.errorText)" }
    }
}

/// The lines of a screen ccrpc cannot read, as the terminal shows them.
private struct ScreenLines: View {
    let lines: [String]

    var body: some View {
        ScrollView(.horizontal) {
            Text(trimmed.joined(separator: "\n"))
                .font(.system(size: 12.5, design: .monospaced))
                .lineSpacing(3)
                .foregroundStyle(Color(red: 0xE6 / 255, green: 0xE6 / 255, blue: 0xE6 / 255))
                .fixedSize()
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(red: 0x1F / 255, green: 0x20 / 255, blue: 0x23 / 255), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var trimmed: [String] {
        let blank = { (s: String) in s.trimmingCharacters(in: .whitespaces).isEmpty }
        guard let a = lines.firstIndex(where: { !blank($0) }), let b = lines.lastIndex(where: { !blank($0) }) else { return [] }
        return Array(lines[a...b])
    }
}
