import SwiftUI

/// Background tasks of a Connect (Tasks.tsx): running ones by kind, each with 停止 (idle only, as on the
/// desktop); the finished ones after them. Opening it asks ccrpc for the list again (`get_tasks`).
struct TasksSheet: View {
    let sessionId: String
    let conn: String
    @Environment(HostModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var error: String?
    @State private var stopping: Set<String> = []

    var body: some View {
        let c = model.connect.conns[conn]
        let tasks = (c?.tasks ?? []).filter { !$0.idle }
        let running = tasks.filter { $0.status == "running" }
        let done = tasks.filter { $0.status != "running" }
        NavigationStack {
            List {
                if tasks.isEmpty {
                    Text("没有后台任务").foregroundStyle(.secondary)
                }
                ForEach(kinds(of: running), id: \.self) { kind in
                    Section(ConnectWords.taskKind(kind).label) {
                        ForEach(running.filter { $0.kind == kind }) { row($0, idle: c?.state == .idle) }
                    }
                }
                if !done.isEmpty {
                    Section("已结束") {
                        ForEach(done) { row($0, idle: false) }
                    }
                }
                if let error {
                    Section { Text(error).foregroundStyle(.red).font(.footnote) }
                } else if !running.isEmpty, c?.state != .idle {
                    Section { Text("空闲时才能停止").foregroundStyle(.secondary).font(.footnote) }
                }
            }
            .navigationTitle("后台任务")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成", systemImage: "checkmark") { dismiss() }.labelStyle(.iconOnly)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .task { _ = await model.connect.command(conn, ["type": "get_tasks"]) }
    }

    private func kinds(of tasks: [ConnTask]) -> [String] {
        let present = Set(tasks.map(\.kind))
        return ConnectWords.taskKindOrder.filter(present.contains) + present.subtracting(ConnectWords.taskKindOrder).sorted()
    }

    private func row(_ t: ConnTask, idle: Bool) -> some View {
        let kind = ConnectWords.taskKind(t.kind)
        let sub = (t.kind == "shell" || t.kind == "monitor") && t.command != nil && t.command != t.name ? t.command : nil
        return HStack(spacing: 10) {
            Image(systemName: kind.symbol).foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(t.name).lineLimit(2)
                if let sub {
                    Text(sub).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(2)
                }
            }
            Spacer(minLength: 4)
            if t.status == "running" {
                Button("停止", role: .destructive) { stop(t) }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(!idle || stopping.contains(t.id))
            } else {
                Text(ConnectWords.taskStatus(t.status)).font(.footnote).foregroundStyle(.secondary)
            }
        }
    }

    private func stop(_ t: ConnTask) {
        stopping.insert(t.id)
        error = nil
        Task {
            let r = await model.connect.command(conn, ["type": "stop_task", "task_id": .string(t.id)])
            stopping.remove(t.id)
            if !r.success { error = "停止失败：\(r.errorText)" }
            _ = await model.connect.command(conn, ["type": "get_tasks"])
        }
    }
}
