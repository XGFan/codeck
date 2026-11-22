import SwiftUI

/// The input card's bottom row, before its button: the permission mode (idle or busy), the model with its effort
/// (idle only), and while background tasks run one button for them all (each kind's icon, how many), so the row keeps
/// its width however many kinds run.
struct ConnectControls: View {
    let c: ConnState
    let sessionId: String
    @Binding var showingTasks: Bool
    @Environment(HostModel.self) private var model

    var body: some View {
        HStack(spacing: 0) {
            modeMenu.layoutPriority(1)
            modelMenu
            Spacer(minLength: 4)
            let running = c.workingTasks
            if !running.isEmpty || showingTasks { tasksButton(running) }
        }
    }

    private var modeMenu: some View {
        let enabled = c.canCommand && (c.state == .idle || c.state == .busy)
        let label = ConnectWords.modeLabel(c.mode)
        return Menu {
            Section("权限模式") {
                Picker("权限模式", selection: Binding(get: { c.mode ?? "" }, set: { setMode($0) })) {
                    ForEach(ConnectWords.modes, id: \.id) { Text($0.label).tag($0.id) }
                }
            }
        } label: {
            FooterMenuLabel(enabled: enabled) {
                if ConnectWords.isRiskyMode(c.mode) {
                    Text(label)
                        .fontWeight(.semibold)
                        .foregroundStyle(Palette.warnText)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 3)
                        .background(Palette.warn.opacity(0.14), in: RoundedRectangle(cornerRadius: 8))
                } else {
                    Text(label)
                }
            }
        }
        .menuOrder(.fixed)
        .tint(Color(uiColor: .label))
        .disabled(!enabled)
    }

    private var modelMenu: some View {
        let family = ConnectWords.modelFamily(c.model)
        let enabled = c.canCommand && c.state == .idle
        return Menu {
            Section("模型 · 也会成为新 Session 的默认") {
                ForEach(ConnectWords.models, id: \.alias) { m in
                    Toggle(m.label, isOn: Binding(get: { m.alias == family }, set: { _ in setModel(m.alias) }))
                }
            }
            if let effort = c.effort {
                Menu {
                    Section("思考等级越高，回答越周全，但更慢、更耗用量。") {
                        ForEach(ConnectWords.efforts, id: \.level) { e in
                            Toggle(e.label, isOn: Binding(get: { e.level == effort }, set: { _ in setEffort(e.level) }))
                        }
                    }
                } label: {
                    Text("Effort")
                    Text(ConnectWords.effortLabel(effort))
                }
            }
        } label: {
            FooterMenuLabel(enabled: enabled) {
                Text([c.model?.name ?? "模型", c.effort.map(ConnectWords.effortLabel)]
                    .compactMap { $0 }
                    .joined(separator: " "))
                    .truncationMode(.tail)
            }
        }
        .menuOrder(.fixed)
        .tint(Color(uiColor: .label))
        .disabled(!enabled)
    }

    private func tasksButton(_ running: [ConnTask]) -> some View {
        Button {
            showingTasks = true
        } label: {
            HStack(spacing: 5) {
                HStack(spacing: -6) {
                    ForEach(ConnectWords.taskKinds(of: running), id: \.self) { kind in
                        Image(systemName: ConnectWords.taskKind(kind).symbol)
                            .font(.system(size: 10, weight: .medium))
                            .frame(width: 22, height: 22)
                            .background(Color(uiColor: .systemGray5), in: Circle())
                            .overlay(Circle().strokeBorder(Color(uiColor: .systemBackground), lineWidth: 1.5))
                    }
                }
                Text("\(running.count)").fontWeight(.semibold).monospacedDigit()
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8)
            .frame(height: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(running.count) 个后台任务")
    }

    private func setMode(_ mode: String) {
        run(["type": "set_mode", "mode": .string(mode)], "切换权限模式")
    }

    private func setModel(_ alias: String) {
        run(["type": "set_model", "model": .string(alias)], "切换模型")
    }

    private func setEffort(_ level: String) {
        run(["type": "set_effort", "level": .string(level)], "切换思考等级")
    }

    private func run(_ cmd: JSONValue, _ what: String) {
        let store = model.connect
        Task {
            let r = await store.command(c.conn, cmd)
            if !r.success { store.setError(sessionId, "\(what)失败：\(r.errorText)") }
        }
    }
}

/// A menu's label in the input card's bottom row: small secondary text with a chevron, 44 high for a finger.
private struct FooterMenuLabel<Content: View>: View {
    let enabled: Bool
    @ViewBuilder let content: Content

    var body: some View {
        HStack(spacing: 4) {
            content
            Image(systemName: "chevron.down").font(.system(size: 9, weight: .semibold))
        }
        .font(.footnote)
        .lineLimit(1)
        .foregroundStyle(enabled ? AnyShapeStyle(.secondary) : AnyShapeStyle(.tertiary))
        .padding(.horizontal, 8)
        .frame(height: 44)
        .contentShape(Rectangle())
    }
}
