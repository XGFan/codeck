import SwiftUI

/// First launch and 「更换 Host」: enter the Host's address, check `/health`, save.
struct HostSetupView: View {
    let current: HostAddress?
    let onConnected: (HostAddress) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var host: String
    @State private var port: String
    @State private var checking = false
    @State private var error: String?
    @FocusState private var hostFocused: Bool

    init(current: HostAddress?, onConnected: @escaping (HostAddress) -> Void) {
        self.current = current
        self.onConnected = onConnected
        _host = State(initialValue: current?.host ?? "")
        _port = State(initialValue: String(current?.port ?? HostAddress.defaultPort))
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 10) {
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 30))
                    .foregroundStyle(Palette.accentText)
                    .frame(width: 64, height: 64)
                    .background(Palette.tint, in: RoundedRectangle(cornerRadius: 18))
                Text("连接 Host").font(.title2.bold()).padding(.top, 6)
                Text("在 CoDeck 桌面打开「局域网访问」，或运行 codeck-host。")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                VStack(spacing: 0) {
                    field("地址") {
                        TextField("192.168.1.20", text: $host)
                            .keyboardType(.numbersAndPunctuation)
                            .textContentType(.URL)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .focused($hostFocused)
                            .submitLabel(.go)
                            .onSubmit(check)
                    }
                    Divider().padding(.leading, 16)
                    field("端口") {
                        TextField(String(HostAddress.defaultPort), text: $port)
                            .keyboardType(.numberPad)
                    }
                }
                .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 18))
                .padding(.top, 14)
                if let error {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(Palette.error)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 4)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 40)
        }
        .scrollBounceBehavior(.basedOnSize)
        .background(Color(uiColor: .systemGroupedBackground))
        .safeAreaInset(edge: .bottom) {
            Button(action: check) {
                HStack(spacing: 8) {
                    if checking { ProgressView().tint(.white) }
                    Text(checking ? "正在连接…" : "连接")
                }
            }
            .buttonStyle(PillButtonStyle())
            .disabled(checking || host.trimmingCharacters(in: .whitespaces).isEmpty)
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
        .navigationTitle(current == nil ? "" : "更换 Host")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if current != nil {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消", systemImage: "xmark") { dismiss() }.labelStyle(.iconOnly)
                }
            }
        }
        .sensoryFeedback(.error, trigger: error) { _, new in new != nil }
        .onAppear { if current == nil { hostFocused = true } }
    }

    private func field<Content: View>(_ label: String, @ViewBuilder content: () -> Content) -> some View {
        HStack {
            Text(label)
            content().multilineTextAlignment(.trailing).font(.body.monospaced())
        }
        .padding(.horizontal, 16)
        .frame(height: 52)
    }

    private func check() {
        guard !checking else { return }
        let address: HostAddress
        do {
            address = try HostAddress.parse(host: host, port: port)
        } catch {
            self.error = error.localizedDescription
            return
        }
        checking = true
        error = nil
        Task {
            defer { checking = false }
            do {
                _ = try await HostClient(address: address).checkHealth()
                onConnected(address)
                dismiss()
            } catch {
                self.error = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }
}

#Preview {
    NavigationStack { HostSetupView(current: nil) { _ in } }
}
