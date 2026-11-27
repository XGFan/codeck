import AVFoundation
import SwiftUI
import UIKit

/// First launch and 「重新扫码」: scan a Host pairing QR code (with DEBUG loopback fallback).
struct HostSetupView: View {
    let current: HostAddress?
    var onPaired: (() -> Void)? = nil

    @Environment(HostModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    @State private var cameraState: CameraState = .checking
    @State private var scanning = true
    @State private var pairing = false
    @State private var pairTask: Task<Void, Never>?
    @State private var error: String?
    @State private var lastInvalid: (code: String, at: Date)?

    #if DEBUG
    @State private var showLoopback = false
    @State private var host: String
    @State private var port: String
    @State private var checkingLoopback = false
    @State private var loopbackError: String?
    #endif

    private enum CameraState {
        case checking
        case authorized
        case denied
        case unavailable
    }

    init(current: HostAddress?, onPaired: (() -> Void)? = nil) {
        self.current = current
        self.onPaired = onPaired
        #if DEBUG
        _host = State(initialValue: "127.0.0.1")
        _port = State(initialValue: String(current?.port ?? HostAddress.defaultPort))
        #endif
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 14) {
                Image(systemName: "qrcode.viewfinder")
                    .font(.system(size: 30))
                    .foregroundStyle(Palette.accentText)
                    .frame(width: 64, height: 64)
                    .background(Palette.tint, in: RoundedRectangle(cornerRadius: 18))
                Text(current == nil ? "连接 Host" : "重新扫码")
                    .font(.title2.bold())
                    .padding(.top, 2)
                Text("在 CoDeck 桌面的「局域网访问」里点「配对新设备」，或在 Host 上运行 codeck-host pair，然后扫描二维码。")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                if let notice = PairingPresentation.setupReplacementNotice(hasCurrentHost: current != nil) {
                    Text(notice)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }

                scannerCard
                    .padding(.top, 6)

                if let error {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(Palette.error)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 4)
                }

                #if DEBUG
                loopbackSection
                    .padding(.top, 8)
                #endif
            }
            .padding(.horizontal, 16)
            .padding(.top, 28)
            .padding(.bottom, 24)
        }
        .scrollBounceBehavior(.basedOnSize)
        .background(Color(uiColor: .systemGroupedBackground))
        .navigationTitle(current == nil ? "" : "重新扫码")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            // Once `/v1/pair` redemption has been sent, lock dismissal until it returns so a
            // consumed pairing key's token is not discarded mid-flight.
            if current != nil && !model.isRedeeming {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消", systemImage: "xmark") {
                        cancelPairing()
                        dismiss()
                    }.labelStyle(.iconOnly)
                }
            }
        }
        .interactiveDismissDisabled(model.isRedeeming)
        .sensoryFeedback(.error, trigger: error) { _, new in new != nil }
        .task { await prepareCamera() }
        .onDisappear {
            scanning = false
            if !model.isRedeeming {
                cancelPairing()
            }
        }
    }

    @ViewBuilder
    private var scannerCard: some View {
        ZStack {
            switch cameraState {
            case .checking:
                ProgressView()
            case .authorized:
                QRScannerPreviewView(isScanning: scanning && !pairing && !model.isPairing) { code in
                    handleScan(code)
                }
                if pairing || model.isPairing {
                    Color.black.opacity(0.55)
                    VStack(spacing: 10) {
                        ProgressView().tint(.white)
                        Text("正在连接…")
                            .font(.subheadline.weight(.semibold))
                            .foregroundStyle(.white)
                    }
                }
            case .denied:
                VStack(spacing: 14) {
                    Image(systemName: "camera.fill")
                        .font(.system(size: 28))
                        .foregroundStyle(.secondary)
                    Text("需要相机权限来扫描 Host 上的二维码。")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                    Button("打开设置") {
                        if let url = URL(string: UIApplication.openSettingsURLString) {
                            openURL(url)
                        }
                    }
                    .buttonStyle(PillButtonStyle(kind: .tinted, height: 44))
                    .frame(maxWidth: 180)
                }
                .padding(24)
            case .unavailable:
                VStack(spacing: 10) {
                    Image(systemName: "video.slash")
                        .font(.system(size: 28))
                        .foregroundStyle(.secondary)
                    Text("这台设备没有可用的相机。")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .padding(24)
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: 270)
        .background(Color(uiColor: .secondarySystemGroupedBackground))
        .clipShape(RoundedRectangle(cornerRadius: 18))
        .overlay(
            RoundedRectangle(cornerRadius: 18)
                .strokeBorder(Palette.hairline, lineWidth: 1)
        )
    }

    private func prepareCamera() async {
        guard AVCaptureDevice.default(for: .video) != nil else {
            cameraState = .unavailable
            return
        }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            cameraState = .authorized
            scanning = true
        case .notDetermined:
            let granted = await AVCaptureDevice.requestAccess(for: .video)
            cameraState = granted ? .authorized : .denied
            scanning = granted
        case .denied, .restricted:
            cameraState = .denied
        @unknown default:
            cameraState = .denied
        }
    }

    private func handleScan(_ code: String) {
        guard !pairing, !model.isPairing else { return }
        let link: PairingLink
        do {
            link = try PairingLink.parse(code)
        } catch {
            // Suppress repeated reports of the same non-CoDeck QR code for 2 seconds.
            let now = Date()
            if let last = lastInvalid, last.code == code, now.timeIntervalSince(last.at) < 2 {
                return
            }
            lastInvalid = (code, now)
            self.error = (error as? LocalizedError)?.errorDescription ?? "这不是 CoDeck 的配对二维码。"
            return
        }

        lastInvalid = nil
        error = nil
        scanning = false
        pairing = true
        pairTask?.cancel()
        pairTask = Task {
            defer {
                pairing = false
                pairTask = nil
            }
            do {
                let paired = try await model.pair(with: link)
                guard !Task.isCancelled else { return }
                if paired {
                    onPaired?()
                    dismiss()
                } else {
                    scanning = true
                }
            } catch is CancellationError {
                // Sheet was dismissed while pairing was in flight; stop silently.
            } catch {
                guard !Task.isCancelled else { return }
                self.error = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
                scanning = true
            }
        }
    }

    private func cancelPairing() {
        pairTask?.cancel()
        pairTask = nil
    }

    #if DEBUG
    private var loopbackSection: some View {
        DisclosureGroup("开发：连接回环 Host", isExpanded: $showLoopback) {
            VStack(spacing: 10) {
                VStack(spacing: 0) {
                    field("地址") {
                        TextField("127.0.0.1", text: $host)
                            .keyboardType(.numbersAndPunctuation)
                            .textContentType(.URL)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .submitLabel(.go)
                            .onSubmit(connectLoopback)
                    }
                    Divider().padding(.leading, 16)
                    field("端口") {
                        TextField(String(HostAddress.defaultPort), text: $port)
                            .keyboardType(.numberPad)
                    }
                }
                .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 14))

                if let loopbackError {
                    Label(loopbackError, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote)
                        .foregroundStyle(Palette.error)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }

                Button(action: connectLoopback) {
                    HStack(spacing: 8) {
                        if checkingLoopback { ProgressView().tint(.white) }
                        Text(checkingLoopback ? "正在连接…" : "连接回环")
                    }
                }
                .buttonStyle(PillButtonStyle(height: 44))
                .disabled(checkingLoopback || host.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding(.top, 10)
        }
        .font(.footnote.weight(.medium))
        .foregroundStyle(.secondary)
        .padding(14)
        .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 18))
    }

    private func field<Content: View>(_ label: String, @ViewBuilder content: () -> Content) -> some View {
        HStack {
            Text(label).foregroundStyle(.primary)
            content().multilineTextAlignment(.trailing).font(.body.monospaced()).foregroundStyle(.primary)
        }
        .padding(.horizontal, 16)
        .frame(height: 48)
    }

    private func connectLoopback() {
        guard !checkingLoopback else { return }
        let address: HostAddress
        do {
            address = try HostAddress.parseLoopback(host: host, port: port)
        } catch {
            loopbackError = error.localizedDescription
            return
        }
        checkingLoopback = true
        loopbackError = nil
        Task {
            defer { checkingLoopback = false }
            do {
                _ = try await HostClient(address: address).checkHealth()
                model.connect(to: address)
                onPaired?()
                dismiss()
            } catch {
                loopbackError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }
    #endif
}

#Preview {
    NavigationStack { HostSetupView(current: nil) }
        .environment(HostModel())
}
