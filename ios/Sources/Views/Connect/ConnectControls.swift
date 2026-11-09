import SwiftUI

/// Under the input box: the permission mode (idle or busy), background tasks, and the model with its effort
/// (idle only), as at the foot of the desktop's input box.
struct ConnectControls: View {
    let c: ConnState
    let sessionId: String
    @Environment(HostModel.self) private var model
    @State private var showingTasks = false

    var body: some View {
        HStack(spacing: 4) {
            modeMenu
            let running = c.runningTasks.count
            if running > 0 || showingTasks {
                Button {
                    showingTasks = true
                } label: {
                    Label("\(running) 个后台任务", systemImage: "square.stack.3d.up")
                }
                .padding(.horizontal, 6)
            }
            Spacer(minLength: 4)
            modelMenu
        }
        .font(.footnote)
        .foregroundStyle(.secondary)
        .buttonStyle(.borderless)
        .sheet(isPresented: $showingTasks) {
            TasksSheet(sessionId: sessionId, conn: c.conn)
        }
    }

    private var modeMenu: some View {
        Menu {
            Picker("权限模式", selection: Binding(get: { c.mode ?? "" }, set: { setMode($0) })) {
                ForEach(ConnectWords.modes, id: \.id) { Text($0.label).tag($0.id) }
            }
        } label: {
            Label(ConnectWords.modeLabel(c.mode), systemImage: "checkmark.shield")
                .foregroundStyle(c.mode == "auto" ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
                .padding(.vertical, 6)
        }
        .disabled(!c.canCommand || !(c.state == .idle || c.state == .busy))
    }

    private var modelMenu: some View {
        let family = ConnectWords.modelFamily(c.model)
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
            HStack(spacing: 4) {
                Text(c.model.map(ConnectWords.modelLabel) ?? "模型").foregroundStyle(.primary)
                if let effort = c.effort { Text(ConnectWords.effortLabel(effort)) }
                Image(systemName: "chevron.up.chevron.down").imageScale(.small)
            }
            .lineLimit(1)
            .padding(.vertical, 6)
        }
        .disabled(!c.canCommand || c.state != .idle)
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
