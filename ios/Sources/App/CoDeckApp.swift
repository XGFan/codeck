import SwiftUI

@main
struct CoDeckApp: App {
    @State private var model = HostModel()
    @Environment(\.scenePhase) private var scenePhase

    private struct PendingPairing {
        var link: PairingLink
        var address: HostAddress
        var hadHost: Bool
    }

    private enum URLPairingPhase {
        case resolving
        case pairing
    }

    /// Resolved external `codeck://pair` link awaiting user confirmation against the actual reachable address.
    @State private var pendingPairing: PendingPairing?
    @State private var urlPairingPhase: URLPairingPhase?
    @State private var resolveTask: Task<Void, Never>?
    @State private var resolveToken: UUID?
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
                if let phase = urlPairingPhase {
                    ZStack {
                        Color.black.opacity(0.25).ignoresSafeArea()
                        VStack(spacing: 12) {
                            HStack(spacing: 10) {
                                ProgressView()
                                Text("正在连接…").font(.subheadline.weight(.semibold))
                            }
                            if phase == .resolving {
                                Button("取消") {
                                    cancelURLResolve()
                                }
                                .font(.subheadline.weight(.medium))
                            }
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
                    get: { pendingPairing != nil },
                    set: { if !$0 { pendingPairing = nil } }
                ),
                presenting: pendingPairing
            ) { pending in
                Button("配对") {
                    let locked = pending
                    pendingPairing = nil
                    startURLPairing(locked.link, at: locked.address)
                }
                Button("取消", role: .cancel) {
                    pendingPairing = nil
                }
            } message: { pending in
                Text(PairingPresentation.openURLConfirmMessage(for: pending.address, hasCurrentHost: pending.hadHost))
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
        if let sessionId = SessionLink.parse(url) {
            if model.host != nil, !model.needsPairing { model.sessionOpenRequest = sessionId }
            return
        }
        // Ignore incoming links while pairing/probing is in progress or another confirmation is open.
        guard !model.isPairing, urlPairingPhase == nil, pendingPairing == nil else { return }
        switch PairingPresentation.classifyOpenURL(url) {
        case .ignore:
            return
        case .invalidPairingLink(let message):
            urlPairingError = message
        case .pairingLink(let link):
            resolveURLPairing(link)
        }
    }

    /// Probes candidate addresses via unauthenticated `/health` first so the confirmation prompt names
    /// the actual reachable `host:port` before any credentials or pairing keys are sent.
    private func resolveURLPairing(_ link: PairingLink) {
        urlPairingPhase = .resolving
        urlPairingError = nil
        resolveTask?.cancel()
        let token = UUID()
        resolveToken = token
        resolveTask = Task {
            defer {
                if resolveToken == token {
                    resolveToken = nil
                    resolveTask = nil
                    if urlPairingPhase == .resolving {
                        urlPairingPhase = nil
                    }
                }
            }
            do {
                let address = try await model.resolve(link)
                guard !Task.isCancelled, resolveToken == token else { return }
                urlPairingPhase = nil
                resolveTask = nil
                resolveToken = nil
                pendingPairing = PendingPairing(link: link, address: address, hadHost: model.host != nil)
            } catch is CancellationError {
                // Cancelled during probe: dismiss silently without showing confirmation or error.
            } catch {
                guard !Task.isCancelled, resolveToken == token else { return }
                urlPairingError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }

    private func cancelURLResolve() {
        resolveToken = nil
        resolveTask?.cancel()
        resolveTask = nil
        if urlPairingPhase == .resolving {
            urlPairingPhase = nil
        }
    }

    private func startURLPairing(_ link: PairingLink, at address: HostAddress) {
        urlPairingPhase = .pairing
        urlPairingError = nil
        Task {
            defer { urlPairingPhase = nil }
            do {
                _ = try await model.pair(with: link, at: address)
            } catch is CancellationError {
                // Ignore cancellation.
            } catch {
                urlPairingError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }
}
