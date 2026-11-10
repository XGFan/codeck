import SwiftUI

/// Above the input box: the permission mode (idle or busy), the model with its effort (idle only), and background
/// tasks while some run.
struct ConnectControls: View {
    let c: ConnState
    let sessionId: String
    @Binding var showingTasks: Bool
    @Environment(HostModel.self) private var model

    var body: some View {
        HStack(spacing: 6) {
            modeMenu
            modelMenu
            let running = c.workingTasks.count
            if running > 0 || showingTasks {
                Button {
                    showingTasks = true
                } label: {
                    GlassPillLabel { Text("\(running) 个任务") }
                }
                .buttonStyle(.plain)
            }
            Spacer(minLength: 0)
        }
    }

    private var modeMenu: some View {
        let enabled = c.canCommand && (c.state == .idle || c.state == .busy)
        return Menu {
            Section("权限模式") {
                Picker("权限模式", selection: Binding(get: { c.mode ?? "" }, set: { setMode($0) })) {
                    ForEach(ConnectWords.modes, id: \.id) { Text($0.label).tag($0.id) }
                }
            }
        } label: {
            GlassPillLabel(enabled: enabled) {
                if c.mode == "auto" {
                    Text(ConnectWords.modeLabel(c.mode)).foregroundStyle(Palette.warnText)
                } else {
                    Text(ConnectWords.modeLabel(c.mode))
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
            GlassPillLabel(enabled: enabled) {
                Text([c.model.map(ConnectWords.modelLabel) ?? "模型", c.effort.map(ConnectWords.effortLabel)]
                    .compactMap { $0 }
                    .joined(separator: " "))
            }
        }
        .menuOrder(.fixed)
        .tint(Color(uiColor: .label))
        .disabled(!enabled)
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
