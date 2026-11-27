@preconcurrency import AVFoundation
import SwiftUI
import UIKit

/// Pure presentation helpers shared by the pairing URL entry and offline banner (unit-tested).
enum PairingPresentation {
    /// Confirmation copy shown when opening a `codeck://pair` link from outside the app.
    static func openURLConfirmMessage(for link: PairingLink, hasCurrentHost: Bool) -> String {
        if hasCurrentHost {
            return "与 Host \(link.firstHost) 配对？会替换当前的 Host。"
        }
        return "与 Host \(link.firstHost) 配对？"
    }

    /// Terminal auth/certificate errors (`needsPairing`) only allow rescanning, not retrying.
    static func showsRetryInOfflineCard(needsPairing: Bool) -> Bool {
        !needsPairing
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
        var onDetect: @MainActor (String) -> Void

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

        func metadataOutput(
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
            let callback = onDetect
            Task { @MainActor in
                callback(value)
            }
        }
    }
}
