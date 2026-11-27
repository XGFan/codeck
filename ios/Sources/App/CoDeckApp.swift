import SwiftUI

@main
struct CoDeckApp: App {
    @State private var model = HostModel()
    @Environment(\.scenePhase) private var scenePhase

    /// Locked link awaiting user confirmation from an external `codeck://pair` URL.
    @State private var pendingLink: PairingLink?
    @State private var pendingHadHost = false
    @State private var pairingFromURL = false
    @State private var urlPairingError: String?

    var body: some Scene {
        WindowGroup {
            Group {
                if model.address != nil {
                    SessionListView()
                } else {
                    NavigationStack {
                        HostSetupView(current: nil)
                    }
                }
            }
            .environment(model)
            .tint(Palette.accentText)
            .overlay {
                if pairingFromURL {
                    ZStack {
                        Color.black.opacity(0.25).ignoresSafeArea()
                        HStack(spacing: 10) {
                            ProgressView()
                            Text("正在连接…").font(.subheadline.weight(.semibold))
                        }
                        .padding(.horizontal, 20)
                        .padding(.vertical, 14)
                        .glass(in: RoundedRectangle(cornerRadius: 16))
                    }
                }
            }
            .onOpenURL { url in
                handleOpenURL(url)
            }
            .alert(
                "配对 Host",
                isPresented: Binding(
                    get: { pendingLink != nil },
                    set: { if !$0 { pendingLink = nil } }
                ),
                presenting: pendingLink
            ) { link in
                Button("配对") {
                    let locked = link
                    pendingLink = nil
                    startURLPairing(locked)
                }
                Button("取消", role: .cancel) {
                    pendingLink = nil
                }
            } message: { link in
                Text(PairingPresentation.openURLConfirmMessage(for: link, hasCurrentHost: pendingHadHost))
            }
            .alert(
                "配对失败",
                isPresented: Binding(
                    get: { urlPairingError != nil },
                    set: { if !$0 { urlPairingError = nil } }
                )
            ) {
                Button("好", role: .cancel) { urlPairingError = nil }
            } message: {
                Text(urlPairingError ?? "")
            }
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active: model.resume()
            case .background: model.pause()
            default: break
            }
        }
    }

    private func handleOpenURL(_ url: URL) {
        // Ignore incoming links while pairing is in progress or another confirmation is open.
        guard !model.isPairing, !pairingFromURL, pendingLink == nil else { return }
        guard let link = try? PairingLink.parse(url) else { return }
        pendingHadHost = model.host != nil
        pendingLink = link
    }

    private func startURLPairing(_ link: PairingLink) {
        pairingFromURL = true
        urlPairingError = nil
        Task {
            defer { pairingFromURL = false }
            do {
                _ = try await model.pair(with: link)
            } catch {
                urlPairingError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }
}
