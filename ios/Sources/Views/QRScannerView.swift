@preconcurrency import AVFoundation
import SwiftUI
import UIKit

/// Pure presentation helpers shared by the pairing URL entry, setup screen, and offline banner (unit-tested).
enum PairingPresentation {
    enum OpenURLClassification: Equatable, Sendable {
        case ignore
        case invalidPairingLink(String)
        case pairingLink(PairingLink)
    }

    /// Distinguishes valid `codeck://pair`, malformed `codeck://pair`, and unrelated URLs.
    static func classifyOpenURL(_ url: URL) -> OpenURLClassification {
        guard PairingLink.isPairingURL(url) else { return .ignore }
        do {
            return .pairingLink(try PairingLink.parse(url))
        } catch {
            return .invalidPairingLink((error as? LocalizedError)?.errorDescription ?? "这不是 CoDeck 的配对二维码。")
        }
    }

    /// Confirmation copy shown when opening a `codeck://pair` link from outside the app,
    /// naming the actual reachable `host:port` discovered during health probing.
    static func openURLConfirmMessage(for address: HostAddress, hasCurrentHost: Bool) -> String {
        if hasCurrentHost {
            return "与 Host \(address.display) 配对？会替换当前的 Host。"
        }
        return "与 Host \(address.display) 配对？"
    }

    /// Notice shown above the QR scanner when a Host is already configured.
    static func setupReplacementNotice(hasCurrentHost: Bool) -> String? {
        hasCurrentHost ? "扫码后会替换当前的 Host。" : nil
    }

    /// Terminal auth/certificate errors (`needsPairing`) only allow rescanning, not retrying.
    static func showsRetryInOfflineCard(needsPairing: Bool) -> Bool {
        !needsPairing
    }

    /// When the device enters `needsPairing`, any open Session detail pops back to the list so the offline card is visible.
    static func shouldReturnToSessionList(needsPairing: Bool) -> Bool {
        needsPairing
    }
}

/// Live AVFoundation QR scanner backed by `AVCaptureMetadataOutput` (`.qr`).
/// Camera session work runs on a dedicated serial queue and stops when the view disappears.
struct QRScannerPreviewView: UIViewRepresentable {
    var isScanning: Bool
    var onDetect: @MainActor (String) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onDetect: onDetect)
    }

    func makeUIView(context: Context) -> CameraPreviewView {
        let view = CameraPreviewView()
        context.coordinator.attach(to: view)
        context.coordinator.setScanning(isScanning)
        return view
    }

    func updateUIView(_ uiView: CameraPreviewView, context: Context) {
        context.coordinator.onDetect = onDetect
        context.coordinator.setScanning(isScanning)
    }

    static func dismantleUIView(_ uiView: CameraPreviewView, coordinator: Coordinator) {
        coordinator.stop()
    }

    final class CameraPreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }

        var previewLayer: AVCaptureVideoPreviewLayer {
            layer as! AVCaptureVideoPreviewLayer
        }

        override init(frame: CGRect) {
            super.init(frame: frame)
            backgroundColor = .black
            previewLayer.videoGravity = .resizeAspectFill
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { fatalError() }
    }

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate, @unchecked Sendable {
        private let session = AVCaptureSession()
        private let sessionQueue = DispatchQueue(label: "dev.joy.codeck.qr-scanner")
        private let lock = NSLock()
        private var configured = false
        private var active = true
        @MainActor var onDetect: @MainActor (String) -> Void

        @MainActor
        init(onDetect: @escaping @MainActor (String) -> Void) {
            self.onDetect = onDetect
        }

        @MainActor
        func attach(to view: CameraPreviewView) {
            view.previewLayer.session = session
            sessionQueue.async { [weak self] in
                self?.configureAndStart()
            }
        }

        @MainActor
        func setScanning(_ scanning: Bool) {
            lock.withLock { active = scanning }
            sessionQueue.async { [weak self] in
                guard let self, self.configured else { return }
                if scanning {
                    if !self.session.isRunning { self.session.startRunning() }
                } else {
                    if self.session.isRunning { self.session.stopRunning() }
                }
            }
        }

        @MainActor
        func stop() {
            lock.withLock { active = false }
            sessionQueue.async { [weak self] in
                guard let self else { return }
                if self.session.isRunning { self.session.stopRunning() }
            }
        }

        private func configureAndStart() {
            if !configured {
                session.beginConfiguration()
                defer { session.commitConfiguration() }
                guard let device = AVCaptureDevice.default(for: .video),
                      let input = try? AVCaptureDeviceInput(device: device),
                      session.canAddInput(input)
                else { return }
                session.addInput(input)

                let output = AVCaptureMetadataOutput()
                guard session.canAddOutput(output) else { return }
                session.addOutput(output)
                output.setMetadataObjectsDelegate(self, queue: sessionQueue)
                if output.availableMetadataObjectTypes.contains(.qr) {
                    output.metadataObjectTypes = [.qr]
                }
                configured = true
            }

            let shouldRun = lock.withLock { active }
            if shouldRun, !session.isRunning {
                session.startRunning()
            }
        }

        nonisolated func metadataOutput(
            _ output: AVCaptureMetadataOutput,
            didOutput metadataObjects: [AVMetadataObject],
            from connection: AVCaptureConnection
        ) {
            guard lock.withLock({ active }) else { return }
            guard let readable = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
                  readable.type == .qr,
                  let value = readable.stringValue,
                  !value.isEmpty
            else { return }
            // Only hop the decoded string across queues; read `onDetect` and re-check scanning state on MainActor.
            Task { @MainActor [weak self] in
                guard let self, self.lock.withLock({ self.active }) else { return }
                self.onDetect(value)
            }
        }
    }
}
