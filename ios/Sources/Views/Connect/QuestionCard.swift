import SwiftUI

/// An AskUserQuestion (QuestionForm.tsx): each question's options, one or several, and 其他 written in;
/// 提交 when every question has an answer.
struct QuestionCard: View {
    let r: QuestionRequest
    let c: ConnState
    @Environment(HostModel.self) private var model
    @State private var picked: [[String]]
    @State private var other: [String]
    /// Single-select: the written answer is chosen instead of an option. Multi-select: 其他 is ticked.
    @State private var useOther: [Bool]
    @State private var busy = false
    @State private var error: String?
    @State private var answered = 0

    init(r: QuestionRequest, c: ConnState) {
        self.r = r
        self.c = c
        _picked = State(initialValue: r.questions.map { _ in [] })
        _other = State(initialValue: r.questions.map { _ in "" })
        _useOther = State(initialValue: r.questions.map { _ in false })
    }

    var body: some View {
        let title = r.questions.count > 1 ? "向你提问 · \(r.questions.count) 个问题" : "向你提问"
        CardShell(icon: "questionmark.bubble", title: title, status: "等待回答", agent: r.agentId) {
            ForEach(Array(r.questions.enumerated()), id: \.offset) { i, q in
                question(i, q)
            }
            HStack {
                if let error { Text(error).font(.footnote).foregroundStyle(.red) }
                Spacer()
                Button("提交", action: submit)
                    .buttonStyle(.borderedProminent)
                    .disabled(busy || !ready)
            }
        }
        .sensoryFeedback(.success, trigger: answered)
        .sensoryFeedback(.selection, trigger: picked)
    }

    private var ready: Bool {
        r.questions.indices.allSatisfy { !picked[$0].isEmpty || Answers.otherChosen(other, useOther, $0) }
    }

    @ViewBuilder
    private func question(_ i: Int, _ q: AskQuestion) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let header = q.header, !header.isEmpty {
                    Text(header)
                        .font(.caption)
                        .foregroundStyle(.tint)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(Color.accentColor.opacity(0.12), in: RoundedRectangle(cornerRadius: 4))
                }
                Text(q.question).font(.subheadline.weight(.medium))
                if q.multiSelect { Text("多选").font(.caption).foregroundStyle(.secondary) }
            }
            ForEach(q.options, id: \.label) { o in
                let on = picked[i].contains(o.label)
                Button {
                    if !q.multiSelect { useOther[i] = false }
                    picked[i] = Answers.togglePick(picked[i], o.label, multi: q.multiSelect)
                } label: {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Mark(on: on, multi: q.multiSelect)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(o.label).foregroundStyle(.primary)
                            if let d = o.description, !d.isEmpty {
                                Text(d).font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(optionBackground(on), in: RoundedRectangle(cornerRadius: 10))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(busy)
            }
            let otherOn = Answers.otherChosen(other, useOther, i)
            HStack(spacing: 8) {
                Button {
                    if q.multiSelect { useOther[i].toggle() } else { chooseOther(i, q) }
                } label: {
                    Mark(on: otherOn, multi: q.multiSelect)
                }
                .buttonStyle(.plain)
                TextField("其他：自己写答案", text: binding(i, q), axis: .vertical)
                    .lineLimit(1...4)
                    .foregroundStyle(!otherOn && !other[i].isEmpty ? .secondary : .primary)
                    .strikethrough(!otherOn && !other[i].trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(optionBackground(otherOn), in: RoundedRectangle(cornerRadius: 10))
            .disabled(busy)
        }
    }

    private func binding(_ i: Int, _ q: AskQuestion) -> Binding<String> {
        Binding(
            get: { other[i] },
            set: { text in
                other[i] = text
                chooseOther(i, q)
            }
        )
    }

    private func chooseOther(_ i: Int, _ q: AskQuestion) {
        useOther[i] = true
        if !q.multiSelect { picked[i] = [] }
    }

    private func optionBackground(_ on: Bool) -> AnyShapeStyle {
        on ? AnyShapeStyle(Color.accentColor.opacity(0.15)) : AnyShapeStyle(Color(uiColor: .tertiarySystemFill))
    }

    private func submit() {
        let answers = Answers.questionAnswers(r.questions, picked: picked, other: other, useOther: useOther)
        let cmd: JSONValue = ["type": "answer_question", "request_id": .string(r.requestId), "answers": .array(answers)]
        Task {
            busy = true
            error = nil
            let resp = await model.connect.answer(c, requestId: r.requestId, cmd)
            busy = false
            if resp.success { answered += 1 } else { error = Answers.error(resp) }
        }
    }
}

/// A radio dot for one answer, a check box when several can be picked.
private struct Mark: View {
    let on: Bool
    let multi: Bool

    var body: some View {
        Image(systemName: multi ? (on ? "checkmark.square.fill" : "square") : (on ? "largecircle.fill.circle" : "circle"))
            .foregroundStyle(on ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
            .imageScale(.medium)
    }
}
