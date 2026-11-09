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
        Form {
            Section {
                LabeledContent("地址") {
                    TextField("192.168.1.20", text: $host)
                        .keyboardType(.numbersAndPunctuation)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .multilineTextAlignment(.trailing)
                        .focused($hostFocused)
                        .submitLabel(.go)
                        .onSubmit(check)
                }
                LabeledContent("端口") {
                    TextField(String(HostAddress.defaultPort), text: $port)
                        .keyboardType(.numberPad)
                        .multilineTextAlignment(.trailing)
                }
            } header: {
                Text("Host")
            } footer: {
                Text("Host 上需运行 codeck-host，并与 iPhone 在同一局域网。")
            }

            if let error {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                Button(action: check) {
                    HStack {
                        Spacer()
                        if checking { ProgressView().padding(.trailing, 4) }
                        Text(checking ? "正在连接…" : "连接").fontWeight(.semibold)
                        Spacer()
                    }
                }
                .disabled(checking || host.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .navigationTitle(current == nil ? "连接 Host" : "更换 Host")
        .navigationBarTitleDisplayMode(current == nil ? .large : .inline)
        .toolbar {
            if current != nil {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
            }
        }
        .sensoryFeedback(.error, trigger: error) { _, new in new != nil }
        .onAppear { if current == nil { hostFocused = true } }
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
